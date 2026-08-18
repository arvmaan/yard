# File Exploration + Review Mode — Feature Design

Branch investigated: `2.5d-projected-map-impl` @ `15884b1`. Read-only research; no code changes made.

## 0. What already exists (so we build on it, not around it)

**Frontend precedent — `web/src/ArtifactInspector.tsx`.** This is the closest thing to a "file viewer" Yard has today. It's a modal dialog (`useModalDialog` hook) that fetches one artifact's content via `fetchArtifactContent()` and renders it two ways via a segmented Preview/Source toggle: Markdown through `react-markdown` (+`remark-gfm`, `rehype-sanitize`), HTML through a sandboxed, DOMPurify-scrubbed `<iframe srcDoc>` with a hand-built CSP. Source view is a bare `<pre><code>` — **no syntax highlighting at all today**. This inspector is the right shape to extend (modal-over-canvas pattern, toolbar with metadata, keyboard-accessible dialog) but it's built for single self-contained documents, not a multi-file tree + diff workflow.

**Backend precedent — `crates/yard-server/src/artifact_service.rs` + routes in `http/mod.rs`.** `ArtifactService` is a content-addressed (SHA-256) blob store keyed by `(project_id, assignment_id, artifact_id)`, exposed via `GET/PUT /api/v1/projects/{p}/assignments/{a}/artifacts/{id}` and `.../artifacts/{id}/content`. Critically, per `yard-domain/src/artifact.rs`, `ArtifactKind` is only `Markdown | Html` — these are agent-authored **reports/documents that get explicitly uploaded**, not a view onto the actual source tree the agent is editing. I grepped the whole repo (`crates/*/src`, `web/src`) for "diff" — zero hits beyond unrelated words ("different," a math `diff` in `mapProjection.ts`). **There is no existing file-tree API, no repo/workspace file-read API, and no diff-generation code anywhere in Yard today.** This is genuinely new backend surface area, not an extension of something half-built.

**What the backend does have that's directly reusable:** `yard-domain/src/project.rs` shows every `Project` is bound to a `workspace_id` (a herdr runtime workspace — i.e., a real directory/checkout the agent operates in), and `yard-domain/src/intervention.rs` defines `SendAssignmentPrompt { command_id, actor, attempt_id, expected_assignment_version, expected_attempt_version, text }`, POSTed to inject a human prompt into a running assignment's agent session (there's an analogous `SendOrchestratorPrompt` and `SendCoordinationNodePrompt`). This prompt-injection path is optimistic-concurrency-controlled (`expected_*_version`) and already wired end-to-end from HTTP → domain → the herdr terminal runtime. **This is the feedback mechanism review comments should ride on** — see §3.

**`web/package.json` audit — nothing to unearth as a hidden gem, meaning we're not blind to an existing library:**
- No syntax highlighter (no Shiki, Prism, highlight.js, CodeMirror, Monaco).
- No diff-rendering library (no `diff`, `jsdiff`, `react-diff-viewer`, `diff2html`).
- No virtualized list/tree component (no `react-window`, `react-virtual`, `react-arborist`).
- What *is* there and relevant: `@xterm/xterm` + `@xterm/addon-fit` (terminal rendering — confirms Yard already renders live agent terminal output somewhere, useful context for the "review mode ↔ live session" relationship), `@xyflow/react` (the node-graph/canvas library, presumably powering the spatial map this branch is about), `dompurify` (already used for sandboxing, reusable for any HTML we render from agent output), `lucide-react` for icons (matches `ArtifactInspector`'s icon usage — keep using it for consistency).
- Conclusion: **new dependencies are justified and expected**, not a shortcut — there's genuinely nothing to repurpose for syntax highlighting, diffing, or tree virtualization.

---

## 1. Comparable products — what to borrow

### Hunk (github.com/modem-dev/hunk) — the owner's explicit reference point

Hunk is a "review-first terminal diff viewer for agent-authored changesets," built on OpenTUI + a diffs library (Pierre), TypeScript. Structurally:
- **Sidebar file list + multi-file review stream**, with split (before/after side-by-side) or stacked (sequential) layout that auto-adapts to available width — this maps directly onto "VS Code style navigation + diff pane."
- **Syntax highlighting** across the diff, not just the tree.
- **The standout: inline AI/agent annotations beside the code**, over a **live, WebSocket-backed session** — the agent process and the review UI share a running connection, so a human's comment isn't just stored, it's visible to (and actionable by) an agent that's still attached to that session. Hunk ships a "skill" the agent installs (`hunk skill path`) so the agent itself knows how to read/respond to annotations placed in the live session, rather than the annotation being a passive database row the agent only sees on its next unrelated turn.
- Git/Jujutsu/Sapling auto-detection, works as a `git difftool`/pager replacement, watch mode for auto-reload as the working tree changes.

**What to take from Hunk specifically:** (a) sidebar-driven multi-file diff stream is the right shape for review mode; (b) annotations should be addressed to a *live* agent channel, not just persisted — this is the strongest argument for routing comments through the existing `SendAssignmentPrompt` intervention path rather than inventing a separate "comments" table the agent has to be taught to poll; (c) split/stacked adaptive layout is a good default for Yard's diff pane given the app is already handling responsive canvas layouts (per `@xyflow/react` usage).

### bb (github.com/get-bb/bb)

An agentic IDE spanning desktop/web/CLI/HTTP API. Most relevant pattern: **work is organized into threads that can be followed live, steered mid-execution, or handed off to another agent**, and one of its showcased views is explicitly "a code review thread, dispatch panel, and task board" — i.e., bb treats code review as one thread type among several, not a bolted-on modal. Architecturally it draws a hard line between a hot-reloading app/UI layer and a stateful, non-hot-reloading server/daemon layer (relevant if Yard's review mode ends up needing its own persistent process, e.g. for live file-watching).

**What to take from bb:** treat "review" as a *view onto an assignment's ongoing work*, addressable the same way other Yard surfaces (terminal output, prompts, artifacts) are addressed — i.e., scoped by `project_id` / `assignment_id` / `attempt_id`, consistent with everything else in `yard-domain`. Don't build review mode as an island with its own identity scheme.

### The "Neovim experience" — concrete picks, not vibes

The owner's ask is explicitly about *file exploration*, not text editing — Yard is not becoming a code editor (no keystroke-level editing of agent-owned files makes sense here; users comment, they don't hand-edit). Three specific Neovim/Telescope ideas transfer, and each earns its place for a distinct reason:

1. **Fuzzy-find file jump (Telescope-style `<leader>ff`).** A single keystroke opens a modal text input that live-filters the *agent-touched file list* (see §2) by fuzzy match on path, with arrow/`Ctrl-n`/`Ctrl-p` to move and Enter to open. Why it matters here specifically: review sessions are triggered by "what did the agent just change," and the touched-file set is usually small (tens, not thousands) — a fuzzy jumper turns "scan the tree" into "type three letters, hit enter," which matches how a reviewer actually thinks ("where's the auth change") rather than how a file tree is organized (by directory).
2. **Keyboard-first modal navigation (normal-mode muscle memory: `j`/`k` to move between files or hunks, `n`/`p` for next/prev diff hunk, `gg`/`G` for top/bottom).** Why here: review is a linear, repetitive task (open file → scan hunks → comment or move on) — exactly Neovim's sweet spot — and a reviewer doing 20 files in a row benefits far more from never leaving the keyboard than from richer editing affordances they don't need.
3. **Instant, non-modal preview on hover/select (`nvim-tree`'s or Telescope's live preview pane) rather than click-to-navigate-away.** Why here: the reviewer's job is comparison and judgment, not editing, so the tree/list should never fully replace the diff pane — selecting a file in the sidebar should update the same diff pane in place, keeping the reviewer's scroll position and mental context intact between files.

**Explicitly out of scope:** modal editing (insert/normal/visual modes for actual text mutation), registers/macros, `:` command-line mode, plugin ecosystem, buffer/window/tab management beyond what a single diff pane needs. Yard doesn't need Neovim's editing model because the artifact being reviewed is agent output, not something the reviewer is typing into.

---

## 2. File tree navigation

**Two-mode tree, not one:**
- **Changed-files mode (default when entering review mode):** a flat or lightly-grouped list of files touched in the current attempt, each row showing +/− line counts and a status glyph (added/modified/deleted/renamed) — VS Code's Source Control panel is the direct model. This is the primary surface; it answers "what did the agent do" without forcing a scan of the full repo tree.
- **Full-tree mode (opt-in toggle):** the complete workspace tree for when a reviewer needs context beyond the diff (e.g., "does a config file already exist that this should have touched"). Collapsible directories, VS Code-style.

**Surfacing "what changed" concretely requires a new backend endpoint**, since nothing in `artifact_service.rs` or elsewhere reads the workspace's actual git state. Proposed: `GET /api/v1/projects/{project_id}/assignments/{assignment_id}/attempts/{attempt_id}/changes`, backed by a new `WorkspaceDiffService` in `yard-server` that shells out to `git diff --name-status` (and `git diff` per-file, on demand) against the workspace directory resolved from the project's `workspace_id`. This mirrors how `ArtifactService` is structured (a thin service wrapping filesystem/process access, called from `http/mod.rs` handlers) rather than introducing a new architectural pattern.

Given Yard already tracks `attempt_id` as a first-class dimension (see `SendAssignmentPrompt`, `Artifact`), diffing naturally scopes to "changes in this attempt" (attempt start commit/ref → working tree or attempt end), which is exactly the unit a reviewer cares about.

## 3. Diff + syntax highlighting for "review mode"

**Diffing:** compute unified diffs server-side via `git diff` (already the source of truth, avoids reimplementing a diff algorithm, and Hunk itself just wraps git/jj/sapling rather than inventing its own). Return structured hunks (old/new line ranges + content) as JSON, not raw patch text, so the frontend can render without a text-format parser.

**Rendering + syntax highlighting, frontend:** since nothing in `package.json` covers this, add **Shiki** (via `shiki`'s browser-friendly WASM/JS build) for highlighting — it's the same engine VS Code uses, ships accurate TextMate-grammar-based highlighting out of the box for a huge language set with no build-time grammar wiring, and is commonly paired with a lightweight custom diff-line renderer rather than a heavier all-in-one component. Pair it with a small, purpose-built diff-line component (render Shiki-highlighted tokens per line, apply add/remove/context background per Yard's existing dark theme) rather than pulling in `react-diff-viewer`-style libraries, which tend to fight custom theming and don't compose well with per-line inline comment affordances (see §3 continued). This keeps the dependency surface small and consistent with the codebase's existing preference for targeted, single-purpose libraries (`dompurify` for sanitization, `rehype-sanitize` for markdown — nothing kitchen-sink).

**Where it lives in the UI:** extend the `ArtifactInspector` pattern rather than replacing it — a new `ReviewInspector` (or generalize `ArtifactInspector` into a shared modal shell) with: header (file path breadcrumb + attempt metadata), left rail (the changed-files tree from §2), main pane (diff view, split/stacked adaptive per Hunk's model), right-aligned comment affordances (below).

## 4. Inline commenting that feeds back to the agent — the precise data path

This is the part most likely to be gotten wrong by treating "comment" as a new first-class entity. Recommendation: **a review comment is not a new object type — it composes into a `SendAssignmentPrompt`.**

Concretely:
1. User clicks a line (or hunk) in the diff pane, types a comment in an inline textarea (VS Code PR-review-style anchor UI).
2. On submit, the frontend does **not** POST to some new `/comments` resource as the primary action. It formats the comment with enough structural context to be unambiguous to the agent — file path, hunk line range, the actual diff snippet, and the comment text — and sends it through the existing prompt-injection path: `POST /api/v1/projects/{p}/assignments/{a}/prompts` (the `SendAssignmentPrompt` handler already in `http/mod.rs`), with `text` containing the formatted comment. This reuses the optimistic-concurrency fields (`expected_assignment_version`, `expected_attempt_version`) that already guard every other write to an assignment, so a comment can't race a state transition the agent just made.
3. Multiple comments queued before submission (a natural VS Code / GitHub PR review pattern — "start a review, add several comments, submit once") should batch into a *single* `SendAssignmentPrompt` call with all comments concatenated/structured, both to avoid spamming the agent's context with N separate turns and to match how a human reviewer actually thinks ("here's my whole pass," not "here's comment #1, wait, here's comment #2").
4. **Comments should still be persisted as structured data independent of the prompt text** — not because the agent needs a separate channel (it doesn't; the prompt *is* the channel), but so the UI can render "resolved/unresolved" state, show comment history per file, and reconstruct what was said if the user reopens review mode later. This argues for a lightweight `ReviewComment` domain type (file path, line/hunk anchor, attempt_id, author, body, created_at, resolved: bool) stored via a small new `yard-store` table — written *alongside* the `SendAssignmentPrompt` call, not instead of it. Anchors should resolve by content-hash-of-surrounding-lines rather than raw line number, so a comment survives the agent's next edit shifting line numbers (a real risk if review happens mid-attempt rather than only at attempt-end).

**The explicit connection point to flag for the parallel chat-UX redesign effort:** if that work is redesigning multi-agent chat toward an iMessage-style model, a review comment *is functionally a targeted chat message to a specific agent*, scoped to a file/line instead of freeform. Both surfaces need to agree on: (a) whether a "message to an agent" is always a `SendAssignmentPrompt`-shaped thing under the hood regardless of which UI produced it, and (b) how a comment's structured context (file, hunk, line anchor) gets represented inside a chat transcript if/when that transcript is the canonical history of "everything said to this agent" — otherwise Yard ends up with two divergent histories of human-to-agent communication (the chat log and the review comment log) that don't reconcile. Recommend the two efforts settle on one shared "message to agent" envelope early, even if review mode ships first with its own minimal comment store, so migration later is a rendering change, not a data migration.

## 5. Phased build plan

**v1 (scoped to be shippable, proves the core loop):**
- Backend: `WorkspaceDiffService` (git-diff-backed), `GET .../attempts/{id}/changes` (file list w/ status) and `GET .../attempts/{id}/diff?path=...` (per-file unified diff as structured JSON).
- Frontend: `ReviewInspector` modal (generalized from `ArtifactInspector`) — changed-files list (§2 changed-files mode only, no full-tree toggle yet), diff pane with Shiki syntax highlighting, stacked layout only (skip split view initially).
- Commenting: inline comment entry per line, submits immediately (no batched "review session" yet) as a formatted `SendAssignmentPrompt`. Minimal `ReviewComment` persistence for resolved/unresolved state and re-render on reopen, line-number anchoring (accept the drift risk short-term).
- Neovim pick #3 only (instant preview on select) — this is nearly free given the modal-over-list structure already exists in `ArtifactInspector`.

**v2:**
- Full workspace tree toggle (§2 full-tree mode).
- Split/side-by-side diff layout as a user preference.
- Batched review sessions ("start review → add N comments → submit once" → single composed prompt).
- Comment anchoring by content hash instead of line number.
- Fuzzy file-jump (Neovim pick #1) — needs the full tree in place first to be worth it.

**v3 / later:**
- Keyboard-first hunk/file navigation (Neovim pick #2) — layer on once the core interaction patterns (which keys, which scope) are validated by real usage.
- Live/watch mode for review mode against an in-progress attempt (Hunk's watch-mode-equivalent) — only worth it once attempts commonly run long enough that reviewing mid-flight is valuable.
- Reconciliation with the chat-UX redesign's message model per §3, once that effort's shape is known.

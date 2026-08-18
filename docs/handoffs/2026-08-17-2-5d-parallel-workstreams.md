# 2.5D follow-up: parallel workstream handoff (2026-08-17)

Continuation of `docs/handoffs/2026-08-15-2-5d-current-state.md`. All branches below are
based on `2.5d-projected-map-impl` and pushed to `origin` (GitHub, `arvmaan/yard`).

## 0. What's fully done, merged into `2.5d-projected-map-impl`

Commit `15884b1` on `2.5d-projected-map-impl` (pushed): switched to real-alpha sprite PNGs
(removed the old feColorMatrix/feComposite alpha-key filter entirely) and fixed a click
"dead zone" regression — growing worker/orchestrator/superintendent hit regions to match
the bigger sprite visuals was intercepting clicks meant for whatever was underneath or
next to them. Reverted to small, click-matched hit regions with the sprite overflowing
past them visually (`inset: -53px` etc.), and relaxed `expectRuntimeLayout`'s overflow
check accordingly. Full gate suite (cargo test, lint, build, unit, full e2e) passed before
this was committed.

## 1. Branches and their real status

| Branch | Status | Gates run? |
|---|---|---|
| `2.5d-herdr-fix` | **Done, verified independently** | cargo test --workspace (63 pass), clippy clean, fmt clean |
| `2.5d-terminal-themes` | **Done, verified independently** | lint/build/unit clean, e2e 72/72 (see §3 caveat on how) |
| `2.5d-knowledge-base` | **WIP, incomplete** | Not run — do not trust as-is |
| `2.5d-terminal-chat-bugs` | **WIP, investigation only, no fix** | Not run |

None of these have been merged into `2.5d-projected-map-impl` or opened as PRs. That's
next — review each on GitHub and merge/PR as you see fit.

## 2. `2.5d-herdr-fix` — done

**Root cause** (confirmed via a red→green regression test, not just theorized): in
`YardOrchestratorService::provision()` (`crates/yard-server/src/yard_orchestrator_service.rs`),
`ensure_session` starts a Herdr session under the globally-unique reserved name
`"yard-orchestrator"`, then `bootstrap_worker` starts an agent under that same name inside
it. If `bootstrap_worker` fails *after* the agent already claimed that name — either
`RuntimeProvisionError::PromptDelivery` (agent live, initial prompt failed) or the
subsequent reconciliation poll timing out after `RUNTIME_IDENTITY_TIMEOUT` (5s) — nothing
ever tore that runtime down. It sat there forever, and every later `provision()` call
failed immediately with Herdr's `agent_name_taken`, requiring a manual
`herdr session list/stop/delete` to clear — the exact recurring bug hit repeatedly this
session.

**Fix**: new `bootstrap_dedicated_worker()` + `retire_orphaned_runtime()` helpers in the
same file. On either failure path, builds a `RuntimeRetirementRequest` straight from the
`WorkerRuntimeBinding` Herdr already returned (every field it needs — terminal_id,
pane_id, workspace_id, etc. — is already on that binding) and calls the *existing*
`RuntimeControl::retire_runtime` primitive (already used elsewhere by
`runtime_cleanup_service.rs`, no new trait method needed) before returning the original
error. Best-effort — retirement failures are only logged, since the caller already has a
provisioning error to surface.

**Tests added**: `retires_the_orphaned_runtime_when_the_initial_prompt_cannot_be_delivered`
and `retires_the_orphaned_runtime_when_the_bootstrapped_worker_never_reconciles`, both
confirmed to fail against the pre-fix code and pass after.

**Verified independently** by me (not just trusting the report): re-ran
`cargo test --workspace --all-targets` in the actual clone — 63/63 pass. Diff reviewed
directly and is a clean, single-file, well-scoped change (208 lines). Ready to merge.

## 3. `2.5d-terminal-themes` — done, plus an infra gotcha worth knowing about

Added 5 named terminal-only color palettes (Nord, Dracula, Solarized Dark/Light, Gruvbox
Dark) as real `ITheme` objects in `web/src/theme.ts`, hex-verified against each theme's
own spec/source rather than recalled from memory (caught one error this way — Gruvbox's
ANSI white is `#7c6f64`, not `#a89984`). Persisted independently of the app's light/dark
toggle via a new `TERMINAL_PALETTE_STORAGE_KEY`, with `'auto'` preserving today's
behavior (deriving colors from the app's own theme) as the default. Picker lives inside
`TerminalSession.tsx`'s own status bar, not the already-busy top command bar. Full diff:
`web/src/theme.ts`, `web/src/TerminalSession.tsx`, `web/src/index.css`,
`web/tests/project-control.spec.ts`.

**The gotcha** (worth remembering for any future frontend verification across multiple
clones of this repo): `web/playwright.config.ts` has a fixed port (5173) and
`reuseExistingServer: !process.env.CI`. There is a long-running `npm run dev` process in
`/home/arv/repos/yard/web` that's been up since Aug 15. When I first ran this branch's
e2e suite from inside the clone directory, Playwright saw port 5173 already occupied and
silently reused *that* server — meaning the test executed against the main repo's stale
code, not this clone's actual changes. That produced one real-looking failure
(`getByLabel('Terminal color theme')` not found — because the main repo's code doesn't
have this feature) which had nothing to do with a bug in the implementation. Re-ran with
a temporary, uncommitted local Playwright config pointed at a private port (5188) and got
a clean 72/72. **If you see an unexplained frontend test failure while multiple clones of
this repo are in play, check for this before debugging the "bug" itself** — `ps aux | grep
vite` to spot a stale server, or just always run e2e against a private, non-default port
when more than one checkout might be alive.

## 4. `2.5d-knowledge-base` — WIP, incomplete

Committed as a clearly-marked WIP commit (`0214f69`, "restyle coordination/knowledge nodes
as ground plinths (incomplete)"), not gated, not to be trusted as done.

What exists: a CSS-only depth-mode restyle of `.coordination-map-node` (the
workstream/knowledge-store marker in `RuntimeCanvas.tsx`) from the old flat circular
glyph-badge into a small tinted "plinth" echoing the buildings' lighter-top/darker-side
axonometric shading.

What's missing: the CSS comment references a `.projected-anchor--coordination-node`
ground-anchor pad "staying visible under it, same as automation nodes" — but that class
does not exist. Nobody wired up the actual ground-anchor-point computation
(`RuntimeCanvas.tsx`'s scene builder, the `unprojectDelta`-based math workers/orchestrators
already use) for coordination nodes. Right now this plinth is just a restyled badge still
floating at its old flat-mode position — it does not yet "stand on the ground" the way the
task actually asked for. **The real remaining work is in `RuntimeCanvas.tsx`'s scene
builder, not more CSS.**

Also unaddressed: the separate "worker image is not properly cropped out" complaint —
worth a quick pixel-sample check against the current sprite (commit `15884b1` already
replaced it with a real-alpha PNG and may have already fixed this; wasn't re-verified).

There's a subagent (`a3ce8f4208dbc72db`) that was mid-task on this when it hit an
account-wide session-limit wall (resets 3:50am America/Vancouver) and got cut off — it may
still be resumable via that agent ID once the limit clears, or just pick the branch up
directly.

## 5. `2.5d-terminal-chat-bugs` — WIP, investigation only

Two bugs were in scope; **neither has a fix yet**. Committed as WIP (`5a8cc7c`,
"scroll-bug repro scaffold, no fix yet").

**Bug 1 — terminal scroll ("I still cant scroll in the terminal mode!!")**: a throwaway
diagnostic test was added (`'TEMP repro: scroll over terminal center point'` in
`project-control.spec.ts`) that logs the DOM hit-chain under the terminal's center point
and — more promisingly — specifically checks whether scroll position survives when new
output streams in while scrolled up into history. That's a live, distinct hypothesis from
the original brief's "something is intercepting the wheel event": **xterm.js may be
auto-scrolling back to the bottom on every write, regardless of whether the user had
already scrolled up** — a very common real-world xterm integration bug, and arguably more
likely than a CSS/z-index interception issue given `TerminalSession.tsx`'s wheel handler
looked structurally correct on read. This hypothesis was never actually confirmed or
refuted — the investigation was cut off by the same session-limit wall before it produced
a result. The diagnostic test should be deleted (it has console.log/screenshot debug
scaffolding, not real assertions) once superseded by a real fix + regression test.

**Bug 2 — chat over-chunking ("we are needlessly chopping up the same message into 5")**:
not started at all. The likely source is still `chunkActivitySegment()` in
`web/src/AgentChatWorkspace.tsx` (~line 131) — see the chat-redesign research doc (§6
below) for the fuller architectural picture of why this exists (it's a symptom of
`AgentChatWorkspace` polling and regex-splitting raw terminal output rather than having a
real message model).

Also hit the same session-limit wall (agent `a064246701a208834`) mid-investigation.

## 6. Research reports (read-only, no code changes) — all three complete

Copied into `docs/research/` alongside this handoff:

- `docs/research/2026-08-17-ui-declutter.md` — top bar/profiles
  bar/control bar audit + consolidation proposal; confirms a dark/light toggle already
  exists and works (`App.tsx:4255-4272`) — it's a discoverability/placement problem, not a
  missing feature; and a grounded Plan-vs-2.5D-mode recommendation (fix 3 specific gaps —
  minimap showing the wrong layout, camera hard-jump on mode switch, resize/connect handles
  not tracking the projected shape — rather than removing Plan mode).
- `docs/research/2026-08-17-chat-redesign.md` — chat UX redesign
  proposal. Core finding: `AgentChatWorkspace.tsx` isn't really a chat system, it's a
  second, lossy transcript of the same terminal Yard already renders live and correctly in
  `TerminalSession.tsx` — which is *why* the chunking bug exists. Recommends retiring the
  single-agent bubble transcript (dock the existing order-composer to the bottom of the
  Terminal view instead) and redesigning `AgentGroupChat.tsx` into a real iMessage-style
  multi-agent thread (per-agent color/avatar identity, append-only messages, `@mention`
  targeting).
- `docs/research/2026-08-17-file-review-mode.md` — VS Code-style file
  exploration + diff/review-mode design doc, informed by reading Hunk's and bb's actual
  READMEs. Key finding: Yard has zero existing diff/tree/syntax-highlighting
  infrastructure, but does already have a live prompt-injection path
  (`SendAssignmentPrompt` in `crates/yard-domain/src/intervention.rs`) that a review
  comment should ride on directly rather than becoming a new, disconnected "comments"
  object the agent has to be taught to poll — and flags that this needs to converge with
  the chat-redesign proposal above on one shared "message to agent" model.

All three were independently reviewed by me (not just the agents' own summaries) and are
specific, well-grounded, and ready for you to read and decide on.

## 7. Immediate next steps, in order

1. Read the three research docs (§6) and decide: declutter approach, chat redesign
   direction, file-review-mode scope. These were explicitly held back from implementation
   pending your call.
2. Review + merge/PR `2.5d-herdr-fix` and `2.5d-terminal-themes` — both done and verified,
   safe to move forward with as-is.
3. Either resume the cut-off subagents (`a3ce8f4208dbc72db` for knowledge-base,
   `a064246701a208834` for terminal-chat-bugs — both hit the same account-wide session
   limit, resets 3:50am America/Vancouver) once it clears, or pick up
   `2.5d-knowledge-base` / `2.5d-terminal-chat-bugs` directly:
   - knowledge-base: wire real ground-anchor computation into `RuntimeCanvas.tsx` for
     coordination nodes (the CSS restyle is already there and looks right, it just isn't
     anchored yet).
   - terminal-chat-bugs: finish confirming/refuting the "xterm auto-scrolls to bottom on
     new output regardless of scroll position" hypothesis for the scroll bug, then fix and
     write a real regression test (delete the diagnostic scaffold); separately, fix
     `chunkActivitySegment` for the chunking bug.
4. Watch for the stale dev server / port-5173 issue (§3) on any future frontend
   verification while multiple clones exist.

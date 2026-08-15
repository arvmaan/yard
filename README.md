<h1 align="center"><code>yard</code></h1>

<p align="center">
  <img alt="Version" src="https://img.shields.io/badge/version-0.1.0-2f6f62?style=flat-square" />
  <img alt="License" src="https://img.shields.io/github/license/arvmaan/yard?style=flat-square" />
  <img alt="Built with Rust and React" src="https://img.shields.io/badge/built%20with-Rust%20%2F%20React-cb6b3d?style=flat-square" />
</p>

<h3 align="center">See the work. Direct the workers.</h3>

Yard turns terminal-based AI agents into a **spatial control plane for real
work**. Projects become places, workers stay visible, and orchestration,
knowledge, and automation have a concrete home on the map.

> **Yard 0.1 is an early preview.** The foundation is usable today, but the
> interface and operating model are still taking shape.

Yard is deliberately inspired by real-time strategy games. We are building
toward the feeling of surveying a living field of work: projects grow into
distinct places, workers move where attention is needed, child agents form
visible trees, and communication paths show coordination as it happens. The
RTS metaphor is an interaction model, not decoration; movement, activity, and
progress should always correspond to real runtime or durable state.

It sits above [Herdr](https://github.com/herdrdev/herdr), preserving the
terminal when you need it while adding the operational layer that terminal
tabs do not provide:

- **One map for active work** - see durable projects plus the selected Herdr
  session's workers, assignments, child agents, and communication state.
- **Move work, not just windows** - allocate or hand off workers, reuse
  operating profiles, send group orders, and resume retained Yard identities.
- **Orchestration at every level** - give each project an orchestrator, group
  projects under workstreams, and coordinate the whole portfolio through a
  dedicated superintendent.
- **Chat or terminal, without losing context** - switch between structured
  agent chat, an embedded terminal, and an external Ghostty window.
- **Knowledge and routines live on the map** - attach revisioned knowledge
  stores and scheduled automations to the orchestrator that owns their scope.
- **Evidence stays distinct from activity** - a running process, delivered
  prompt, or successful automation does not silently become "completed work."

| Component | v0.1 preview status |
|---|---|
| Interface | local browser app; light and dark themes |
| Runtime | Herdr 0.8.0 |
| Persistence | local SQLite database and managed files |
| Distribution | source only; no packaged or signed release |

## Contents

- [The core idea](#the-core-idea-the-terminal-is-not-the-control-plane)
- [How Yard works](#how-yard-works)
- [Quick start](#quick-start)
- [First run](#first-run)
- [Projects and workers](#projects-and-workers)
- [Orchestration](#orchestration)
- [Knowledge and automations](#knowledge-and-automations)
- [Architecture](#architecture)
- [Configuration](#configuration)
- [Development and verification](#development-and-verification)
- [Current limits](#current-limits)

## The core idea: the terminal is not the control plane

Agent runtimes are good at running agents. Once several projects and sessions
are active, however, terminal topology stops answering the important
questions:

- Which worker owns which objective?
- What is active, blocked, or waiting for manual intervention?
- Which child agents belong to which parent?
- What context should move when work is handed off?
- Which orchestrator can coordinate across project boundaries?
- What evidence actually proves the work is done?

Yard makes those relationships durable and operable without replacing the
runtime underneath them.

```text
                         Superintendent
                               |
                  +------------+------------+
                  |                         |
          Project orchestrator      Workstream orchestrator
                  |                         |
           workers + children       projects + knowledge
                  |                         |
             Herdr sessions              automations
```

Herdr remains authoritative for live processes, panes, and terminals. Yard
owns project identity, assignments, orchestration, routing, artifacts,
knowledge snapshots, automation schedules, and completion evidence.

## How Yard works

```text
     Herdr sessions and terminals
                  |
                  v
      discovery + reconciliation
                  |
                  v
    Yard's durable operational model
                  |
        +---------+---------+
        |                   |
        v                   v
  spatial map       chat / terminal views
        |                   |
        +---------+---------+
                  |
                  v
       explicit commands to Herdr
```

Yard continuously observes Herdr and reconciles live runtime resources with
its durable model. Commands such as project creation, allocation, prompting,
handoff, and session termination are explicit, versioned operations. If Yard
cannot determine whether a runtime command landed, it records an ambiguous
outcome instead of guessing or retrying destructive work.

## Quick start

Prerequisites:

- Git and a native C build toolchain;
- Rust 1.85 or newer;
- Node.js `^20.19.0` or `>=22.12.0` and npm;
- Herdr 0.8.0 available as `herdr`;
- an authenticated agent provider supported by Herdr, such as Codex or Claude;
- a running Herdr session containing the agents you want to operate.

Install Herdr if needed:

```sh
curl -fsSL https://herdr.dev/install.sh | sh
herdr --version
```

Clone Yard and install its dependencies:

```sh
git clone https://github.com/arvmaan/yard.git
cd yard
cargo fetch

cd web
npm ci
cd ..
```

Start the control-plane service:

```sh
cargo run -p yard-server
```

In another terminal, from the repository root, start the web client:

```sh
cd web
npm run dev
```

Open <http://127.0.0.1:5173>. The API listens on
<http://127.0.0.1:4317>.

Yard will display the sessions reported by Herdr. If there are none, start one
with Herdr before continuing.

## First run

1. Select a Herdr session from the top bar. Its observed workers appear on the
   map.
2. Create a reusable worker profile for new orchestrators. An observed
   workspace can instead be adopted with one of its existing workers.
3. Create a project and give it a working directory and objective. New
   projects receive a dedicated project orchestrator.
4. Drag unassigned workers into the project, or select several workers and send
   a group order.
5. Open a worker in **Chat** for structured messages or **Terminal** for the
   full session. Use **Open in Ghostty** when an external terminal is preferable.
6. Start the **Superintendent** to collect structured project updates and route
   work across project orchestrators.
7. Right-click empty map space to add a workstream orchestrator or knowledge
   store. Automations are attached to the orchestrator whose scope they serve.
8. When work claims to be finished, review the evidence and record a manual
   completion receipt. Runtime state alone is never treated as proof.

## Projects and workers

Projects are persistent territories on a free-form map. Their boundaries,
placement, color, orchestrator, assignments, and relationships survive runtime
restarts.

Workers can be:

- discovered from existing Herdr sessions;
- created from a reusable profile;
- assigned or handed off by dragging them between projects;
- prompted individually or as a selected group;
- resumed under the same Yard identity, with a replacement runtime when needed;
- opened in Ghostty using the exact bound terminal;
- ended explicitly when their session is no longer needed.

Live Codex and Claude subagents are shown as children connected to their parent
terminal. Exited subagents are removed from the active tree.

## Orchestration

Every project has exactly one project orchestrator. Yard can also provision:

- a **Superintendent**, the dedicated top-level Herdr session for portfolio
  status and cross-project routing;
- **workstream orchestrators**, which coordinate selected groups of projects;
- structured project updates that separate the last action, remaining work,
  current state, and required owner action.

Connection paths reflect observed communication state: idle links remain
neutral, active exchanges turn green, and failed exchanges turn red. The
visual state follows persisted routing attempts; it is not decorative activity.

## Knowledge and automations

A knowledge store is a map node attached to one or more projects. Its
orchestrator asks those projects for standardized context, stores immutable
source snapshots, and records the combined revision without overwriting the
inputs.

Automations are recurring prompts attached to an orchestration scope:

- the superintendent for portfolio routines;
- a project orchestrator for project-specific work;
- a workstream orchestrator for a selected group of projects.

Schedules are daily and timezone-aware in v0.1. Every run has a durable record,
and successful delivery means only that the prompt reached its target.

## Architecture

Yard is a Rust workspace with a React client:

```text
crates/
  yard-domain/    provider-neutral commands and durable entities
  yard-herdr/     Herdr discovery, lifecycle, output, and terminal adapter
  yard-store/     SQLite persistence and migrations
  yard-server/    loopback REST and WebSocket control plane
web/
  src/            React map, inspectors, chat, and terminal workspaces
  tests/          Playwright acceptance coverage
scripts/
  live-herdr-smoke.sh
  live-v1-acceptance.sh
```

The server binds only to loopback while Yard has no authentication layer.
SQLite runs in WAL mode, and one Yard process exclusively owns a database.

## Configuration

| Variable | Purpose | Default |
|---|---|---|
| `YARD_BIND` | API socket; loopback addresses only | `127.0.0.1:4317` |
| `YARD_HERDR_BIN` | Herdr executable | `herdr` |
| `YARD_GHOSTTY_BIN` | Ghostty executable | macOS app bundle, then `ghostty` |
| `YARD_DATABASE_PATH` | SQLite control database | `$XDG_DATA_HOME/yard/yard.sqlite3` or `$HOME/.local/share/yard/yard.sqlite3` |
| `YARD_ARTIFACT_PATH` | managed artifact bytes | `artifacts/` beside the database |
| `YARD_COORDINATION_PATH` | managed workstream directories | `coordination/` beside the database |
| `YARD_KNOWLEDGE_PATH` | managed knowledge snapshots | `knowledge/` beside the database |
| `YARD_ORCHESTRATOR_CWD` | working directory for the superintendent | process working directory |
| `YARD_API_TARGET` | Vite proxy API target | `http://127.0.0.1:4317` |

Stop Yard before copying its database and managed directories for backup.
Managed coordination and knowledge paths reject symlink traversal.

## Development and verification

Run the Rust gates from the repository root:

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
bash -n scripts/live-herdr-smoke.sh
bash -n scripts/live-v1-acceptance.sh
```

Run the web gates from `web/`:

```sh
npm ci
npx playwright install chromium
npm run lint
npm run build
npm run test:e2e
```

`scripts/live-herdr-smoke.sh` creates real temporary Herdr resources. The
historical `live-v1-acceptance.sh` harness also launches authenticated Codex
workers with `--yolo`, can consume model quota, and defaults to a one-hour run.
Read both scripts before running them.

See [CONTRIBUTING.md](CONTRIBUTING.md) for change and review expectations and
[SECURITY.md](SECURITY.md) for the local trust boundary.

## Current limits

Yard 0.1 is an early, single-user local tool:

- there is no authentication, authorization, TLS, or remote deployment model;
- Herdr 0.8.0 is the only runtime adapter;
- there are no packaged or signed binaries;
- interrupted project creation, allocation, or handoff can retain a safety
  reservation without a self-service cancel or resolve workflow;
- knowledge collection and automation delivery are transport events, not
  completion evidence.

See [CHANGELOG.md](CHANGELOG.md) for the v0.1 feature summary.

## License

[MIT](LICENSE)

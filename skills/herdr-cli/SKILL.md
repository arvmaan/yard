---
name: herdr-cli
description: Operate the herdr terminal-workspace CLI. Use when creating or inspecting Herdr workspaces, worktrees, panes, and agent processes, or when another skill needs exact Herdr command patterns.
---

# herdr CLI

`herdr` is a terminal workspace manager for AI coding agents. It manages
workspaces, tabs, panes, git worktrees, and the interactive agent processes
running inside panes, all through a local socket API that the `herdr` binary
talks to. Every command returns JSON on stdout, including errors.

Run `herdr <subcommand> --help` whenever exact flags matter.

## Mental model

- **Workspace**: a top-level project container.
- **Worktree workspace**: a workspace rooted in an isolated git worktree.
- **Pane**: a terminal pane inside a tab.
- **Agent**: an interactive CLI coding agent running inside a pane.

## Discovering current state

```bash
herdr workspace list
herdr workspace get <id>
herdr worktree list
herdr agent list
herdr agent get <name-or-pane-id>
herdr pane read <pane-id>
```

`agent_status` is one of `idle`, `working`, `blocked`, `done`, `unknown`.

## Choosing the worker's workspace

When an orchestrator is already running in Herdr, create delegated workers as
tabs in that orchestrator's workspace. Resolve the parent once:

```bash
orchestrator_pane="$(herdr pane current)"
parent_workspace_id="$(
  printf '%s' "$orchestrator_pane" | jq -er '.result.pane.workspace_id'
)"
```

Do not hardcode a workspace ID, infer one from the repository name, or follow
UI focus after delegation begins.

Prefer one git worktree per writing agent:

```bash
repo_root=/path/to/repo
checkout=/path/to/checkouts/<worker-name>
branch=<new-branch-name>

git -C "$repo_root" worktree add -b "$branch" "$checkout" <base-ref>

created_tab="$(
  herdr tab create \
    --workspace "$parent_workspace_id" \
    --cwd "$checkout" \
    --label "<Human label>" \
    --no-focus
)"
pane_id="$(printf '%s' "$created_tab" | jq -er '.result.root_pane.pane_id')"
tab_id="$(printf '%s' "$created_tab" | jq -er '.result.tab.tab_id')"
```

Verify the returned workspace IDs and CWD before starting an agent. For
read-only work, a plain tab is enough:

```bash
herdr tab create \
  --workspace "$parent_workspace_id" \
  --cwd <path> \
  --label "<label>" \
  --no-focus
```

Create a separate workspace only when explicitly requested or when the
orchestrator is not running in Herdr.

## Starting an agent in a pane

```bash
herdr agent start <name> --kind <kind> --pane <pane_id> [--timeout <ms>] [-- <agent-args>...]
```

Use the runtime default unless the user or an explicit runtime profile
selected additional permission or sandbox arguments. Do not infer bypass
arguments from worktree isolation, task scope, cadence, or provider.

## Sending a prompt

```bash
herdr agent prompt <name> "<prompt text>" --wait --until idle --until blocked --timeout <ms>
```

Without `--wait`, submission returns immediately. A wait timeout means the
CLI stopped waiting, not that the agent failed; check `herdr agent get`.

For long tasks, submit without waiting and poll:

```bash
herdr agent prompt worker1 "do the thing"
herdr agent get worker1
herdr agent read worker1
```

Agent narration is for humans. Require a named output file when structured
results matter.

## Cleanup

Close only the worker tab, then remove an isolated checkout only after review:

```bash
herdr tab close <tab_id>
git -C <repo_root> worktree remove <checkout_path>
```

Never close the parent workspace as worker cleanup. Never remove a worktree
containing unreviewed changes.

## Common pitfalls

- A target pane must be at an interactive shell prompt before `agent start`.
- `--wait` may observe an already-running turn; inspect status before assuming
  a specific prompt completed.
- A CLI timeout is not an agent failure.
- Each worktree needs its own branch.

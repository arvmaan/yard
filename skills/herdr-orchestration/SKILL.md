---
name: herdr-orchestration
description: Run independent, resumable work lanes in Herdr tabs attached to the originating workspace. Use after orchestration identifies multiple substantial deliverables that need durable terminals or separate worktrees. Not for quick research, one-off commands, or tightly coupled edits.
---

# herdr Orchestration

A repeatable pattern for fanning a task out across several `herdr`-managed
agent tabs in the originating workspace, one per sub-task, then collecting
their results. This skill assumes the `herdr-cli` skill's command reference;
read that first if you haven't used `herdr` before.

Use the `orchestrate-work` skill first to decide whether durable Herdr workspaces
are warranted. Keep bounded read-only investigations in ordinary workers when
the active harness provides them.

## When this applies

Use this when:

- The task decomposes into **N sub-tasks that don't need to see each other's
  work while running** (they can synthesize at the end, but don't need
  mid-flight coordination).
- At least some sub-tasks may **write files** (code, notes, plans), so giving
  each one its own git worktree avoids collisions.
- You want a **durable, inspectable trail**: each sub-task's agent leaves a
  terminal transcript and, ideally, a written artifact you can read back.

Don't use this for a single tightly-scoped edit, or for sub-tasks that need to
negotiate with each other in real time (that wants a single agent with full
context, not N isolated ones).

## Step 1: Decompose

Before touching `herdr`, write down the sub-task list explicitly:

- One line per sub-task: what it covers, what "done" looks like, what
  artifact it should produce.
- Decide whether sub-tasks need write access to a shared repo. If yes, each
  one needs its own git worktree (isolated branch + checkout). If sub-tasks
  are read-only research, plain tabs are enough and cheaper to set up.
- Decide the base ref every worktree should branch from (e.g. the repo's main
  branch) and a naming scheme for branches/labels so they're recognizable in
  `herdr tab list` later (e.g. `proj/<area>` branches, `"<Project>:
  <Area>"` labels).

## Step 2: Preserve the parent workspace

When the orchestrator itself is running in Herdr, delegated agents belong to
that orchestrator's workspace. Resolve the parent workspace once, before
creating any workers:

```bash
orchestrator_pane="$(herdr pane current)"
parent_workspace_id="$(
  printf '%s' "$orchestrator_pane" | jq -er '.result.pane.workspace_id'
)"
```

Do not hardcode a workspace ID, use the currently focused workspace, or infer
a workspace from the repository name. The parent pane's workspace is the
source of truth. Keep the captured ID for the entire fleet so focus changes
while workers start cannot redirect later workers.

Only create a separate Herdr workspace when the user explicitly requests one
or the orchestrator is not running in Herdr.

## Step 3: Provision one tab per sub-task

For repo-writing sub-tasks, create a worktree per sub-task:

```bash
repo_root=/path/to/repo
checkout_parent=/path/to/checkouts
base_ref=<base-ref>
area=area-a
branch="proj/${area}"
checkout="${checkout_parent}/${area}"

git -C "$repo_root" worktree add -b "$branch" "$checkout" "$base_ref"

created_tab="$(
  herdr tab create \
    --workspace "$parent_workspace_id" \
    --cwd "$checkout" \
    --label "Project: ${area}" \
    --no-focus
)"
pane_id="$(printf '%s' "$created_tab" | jq -er '.result.root_pane.pane_id')"
tab_id="$(printf '%s' "$created_tab" | jq -er '.result.tab.tab_id')"
```

Record each lane's branch, checkout path, pane ID, and tab ID together. You
need the pane ID in the next step and the tab ID for later lifecycle
decisions. Before starting the agent, verify both returned workspace IDs equal
`parent_workspace_id` and the root pane's returned CWD equals `checkout`; stop
on any mismatch instead of moving or allocating the pane. Repeat the
self-contained sequence per lane; independent lanes can run as separate shell
calls in the same parallel batch.

For read-only sub-tasks, create a tab in the captured parent workspace; no
worktree is needed:

```bash
herdr tab create \
  --workspace "$parent_workspace_id" \
  --cwd <path> \
  --label "<label>" \
  --no-focus
```

## Step 4: Start an agent in each pane

Pick one agent kind for the fleet (or mix kinds if some sub-tasks suit a
different model/tool better). Give each a unique, memorable name tied to its
sub-task:

```bash
herdr agent start area-a --kind <kind> --pane <pane_id_a>
herdr agent start area-b --kind <kind> --pane <pane_id_b>
herdr agent start area-c --kind <kind> --pane <pane_id_c>
```

Use the runtime default unless the user or an explicit runtime profile selected
additional provider-specific arguments. Do not infer token-spend permission or
bypass arguments from worktree isolation, task scope, cadence, or provider.

## Step 5: Brief each agent with a self-contained prompt

This is the step that determines output quality. Each agent starts with
**zero conversation context** — it doesn't know why the fleet exists, what
the other sub-tasks are, or what "good" looks like unless you tell it. Treat
this like briefing a new contractor, not like continuing a chat.

A good per-agent prompt includes:

1. **What this sub-task covers and what it does NOT cover** (the boundary
   with neighboring sub-tasks, so it doesn't duplicate or drift into their
   territory).
2. **The concrete facts you already know**, inlined directly — don't say "go
   read the report," paste the actual key facts, numbers, IDs, and open
   questions relevant to this slice. Point at source files/docs for depth,
   but don't make the agent hunt for the load-bearing facts.
3. **What "don't do yet" looks like**, if applicable (e.g. "catch up and plan,
   don't write code yet").
4. **The deliverable, and where to put it.** Terminal narration is for a
   human watching live, not a structured return value. Tell the agent to
   write its findings to a named file in its own cwd (e.g. `NEXT_STEPS.md`,
   `findings.json`) so you can `Read` it back deterministically afterward.
5. **Any environment quirks specific to that worktree** (missing checkouts,
   setup commands, known-stale reference material to distrust).

Require the deliverable to include conclusions, evidence, files changed,
checks run and their observed results, unresolved risks, and the recommended
next action. For write lanes, also require a focused commit, its SHA, and a
clean worktree, or an explicit no-change conclusion.

Send prompts without `--wait` when kicking off several at once, so you don't
serialize on each other:

```bash
herdr agent prompt area-a "<self-contained brief>"
herdr agent prompt area-b "<self-contained brief>"
herdr agent prompt area-c "<self-contained brief>"
```

## Step 6: Poll, don't block

Long research/planning turns can run for minutes. Poll status instead of
holding a long `--wait`:

```bash
for name in area-a area-b area-c; do
  echo "$name: $(herdr agent get "$name" | jq -r '.result.agent.agent_status')"
done
```

A CLI `--wait --timeout` call that times out means the *wait* gave up, not
that the agent failed — always confirm via `herdr agent get` before treating
a sub-task as errored. Re-poll every 30-90s for tasks expected to take single
digit minutes; longer for open-ended research.

## Step 7: Collect and synthesize

Once every sub-task's agent reaches `idle`/`done`:

- Read each sub-task's deliverable file directly from its worktree's checkout
  path; don't rely on scraping terminal narration.
- Skim `herdr agent read <name>` only if you need to see *how* it got there.
- Review each write lane's diff and commit, run the relevant checks, and
  integrate accepted commits in an explicit order.
- Synthesize across sub-tasks yourself.

## Step 8: Clean up

Decide per sub-task whether its worktree becomes real follow-up work or was
purely exploratory:

```bash
herdr tab close <tab_id>
git -C <repo_root> worktree remove <checkout_path>
```

Don't remove a worktree with uncommitted changes you haven't reviewed yet.
Tab closure and worktree removal are explicit lifecycle decisions, not
routine cleanup.

## Common failure modes

- **Vague briefs → generic output.** Front-load the facts.
- **Forgetting the boundary.** State what each lane excludes.
- **Over-trusting stale reference docs.** Call out known-stale material.
- **Skipping a written result.** Require a named artifact for structured
  conclusions.
- **Creating sibling workspaces instead of sibling tabs.** Capture the parent
  workspace once and pass it to every `tab create`.

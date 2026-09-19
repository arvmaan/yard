import { describe, expect, it } from "vitest"
import {
  groupRuntimeLensEntries,
  runtimeLensEntryKey,
} from "./runtimeLensTargets"
import type { AgentWorkspaceTarget } from "./AgentWorkspaceContext"
import type { RuntimeInventory, RuntimeLensEntry } from "./types"

function entry(
  terminalId: string,
  classification: RuntimeLensEntry["classification"],
  workerId: string | null = null,
): RuntimeLensEntry {
  return {
    classification,
    session: "alpha",
    workspace_id: "workspace-1",
    tab_id: `tab-${terminalId}`,
    pane_id: `pane-${terminalId}`,
    terminal_id: terminalId,
    observed_at_unix_ms: "1",
    binding_last_observed_at_unix_ms: workerId ? "1" : null,
    snapshot_current: true,
    reason: classification.replaceAll("_", " "),
    worker_id: workerId,
    profile_name: null,
    availability: null,
    provider: classification === "topology_only_shell_pane" ? null : "codex",
    display_provider: null,
    name: terminalId,
    label: null,
    status: "idle",
    focused: false,
    launch_pending: false,
    interactive_ready: classification !== "topology_only_shell_pane",
  }
}

const inventory: RuntimeInventory = {
  adapter: "herdr",
  session: "alpha",
  runtime_version: "1",
  protocol: 1,
  observed_at_unix_ms: 1,
  focus: { workspace_id: null, tab_id: null, pane_id: null },
  workspaces: [{
    runtime_id: "workspace-1",
    order: 0,
    label: "Workspace 1",
    focused: true,
    active_tab_id: "",
    pane_count: 4,
    tab_count: 4,
    status: "idle",
    tokens: {},
    worktree: null,
  }],
  tabs: [],
  panes: [],
  workers: [],
  child_agents: [],
}

function controlledTarget(
  workerId: string,
  identity: RuntimeLensEntry,
): AgentWorkspaceTarget {
  return {
    session: identity.session,
    workspaceId: identity.workspace_id,
    tabId: identity.tab_id,
    paneId: identity.pane_id,
    terminalId: identity.terminal_id,
    target: {
      kind: "assignment",
      assignment: { worker: { id: workerId } },
    },
  } as AgentWorkspaceTarget
}

describe("runtime lens grouping", () => {
  it("keeps unassigned agents and shell panes searchable", () => {
    const agent = entry("agent-1", "unassigned_herdr_agent")
    const shell = entry("shell-1", "topology_only_shell_pane")

    expect(
      groupRuntimeLensEntries([shell, agent], inventory, "agent-1")
        .flatMap((group) => group.entries)
        .map(runtimeLensEntryKey),
    ).toEqual([runtimeLensEntryKey(agent)])
    expect(
      groupRuntimeLensEntries([shell, agent], inventory, "shell-1")
        .flatMap((group) => group.entries)
        .map(runtimeLensEntryKey),
    ).toEqual([runtimeLensEntryKey(shell)])
  })

  it("suppresses linked rows only by rendered worker or exact runtime identity", () => {
    const byWorker = entry("linked-worker", "linked_yard_worker", "worker-1")
    const byRuntime = entry("linked-runtime", "linked_yard_worker", "worker-2")
    const leftover = entry("linked-leftover", "linked_yard_worker", "worker-3")
    const unassigned = entry("agent-1", "unassigned_herdr_agent")
    const controlled = [
      controlledTarget(
        "worker-1",
        entry("controlled-worker", "linked_yard_worker", "worker-1"),
      ),
      controlledTarget("worker-other", byRuntime),
    ]

    expect(
      groupRuntimeLensEntries(
        [byWorker, byRuntime, leftover, unassigned],
        inventory,
        "",
        controlled,
      )
        .flatMap((group) => group.entries)
        .map(runtimeLensEntryKey),
    ).toEqual([runtimeLensEntryKey(unassigned), runtimeLensEntryKey(leftover)])
  })
})

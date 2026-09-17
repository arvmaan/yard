import { useState } from 'react'
import {
  CircleAlert,
  ExternalLink,
  LoaderCircle,
  MessageSquareText,
  RefreshCw,
  SquareTerminal,
} from 'lucide-react'
import {
  agentWorkspaceKey,
  terminalLeaseKey,
  useAgentWorkspace,
  type AgentWorkspaceTarget,
} from './AgentWorkspaceContext'
import { YardApiError, openTerminalInGhostty } from './api'
import {
  resolveRuntimeCapabilities,
  runtimeCapabilityDetail,
  runtimeCapabilityLabel,
  runtimeCapabilityStatus,
} from './runtimeCapabilities'
import type {
  Assignment,
  CoordinationNode,
  CoordinationNodeRoute,
  Project,
  RuntimeInventory,
  YardOrchestrator,
  YardOrchestratorRoute,
} from './types'

type InterventionTarget =
  | { kind: 'assignment'; assignment: Assignment }
  | { kind: 'orchestrator'; project: Project }
  | { kind: 'yard-orchestrator'; orchestrator: YardOrchestrator }
  | { kind: 'coordination-node'; node: CoordinationNode }

function shellQuote(value: string) {
  return /^[A-Za-z0-9_./:-]+$/.test(value)
    ? value
    : `'${value.replaceAll("'", "'\\''")}'`
}

export function WorkerInterventions({
  onCoordinationChange,
  onCoordinationNodeChange,
  inventory,
  onRefresh,
  projects,
  routes,
  snapshotCurrent,
  target,
}: {
  onCoordinationChange?: (route: YardOrchestratorRoute) => void
  onCoordinationNodeChange?: (route: CoordinationNodeRoute) => void
  inventory: RuntimeInventory | null
  onRefresh?: () => void
  projects?: Project[]
  routes?: Array<YardOrchestratorRoute | CoordinationNodeRoute>
  snapshotCurrent: boolean
  target: InterventionTarget
}) {
  void onCoordinationChange
  void onCoordinationNodeChange
  void routes
  const assignment =
    target.kind === 'assignment' ? target.assignment : null
  const project =
    target.kind === 'orchestrator' ? target.project : null
  const yardOrchestrator =
    target.kind === 'yard-orchestrator' ? target.orchestrator : null
  const coordinationNode =
    target.kind === 'coordination-node' ? target.node : null
  const projectId = assignment?.project_id ?? project?.id ?? ''
  const assignmentId = assignment?.id ?? null
  const workerId =
    assignment?.worker.id ??
    project?.orchestrator.id ??
    yardOrchestrator?.worker?.id ??
    coordinationNode?.worker?.id ??
    ''
  const runtime =
    assignment?.worker.runtime ??
    project?.orchestrator.runtime ??
    yardOrchestrator?.worker?.runtime ??
    coordinationNode?.worker?.runtime
  const terminalId = runtime?.terminal_id
  const runtimeSession = runtime?.session
  const targetKey = agentWorkspaceKey(target)
  const label =
    target.kind === 'orchestrator'
      ? `${project?.name ?? 'Project'} orchestrator`
      : target.kind === 'yard-orchestrator'
        ? 'Superintendent'
        : target.kind === 'coordination-node'
          ? coordinationNode?.name ?? 'Workstream orchestrator'
          : assignment?.profile_name ?? 'Worker'
  const terminalTarget =
    target.kind === 'assignment' && assignmentId
      ? {
          kind: 'assignment' as const,
          projectId,
          assignmentId,
        }
      : target.kind === 'orchestrator'
        ? {
            kind: 'orchestrator' as const,
            projectId,
            workerId,
          }
        : workerId
          ? target.kind === 'coordination-node' && coordinationNode
            ? {
                kind: 'coordination-node' as const,
                nodeId: coordinationNode.id,
                workerId,
              }
            : {
                kind: 'yard-orchestrator' as const,
                workerId,
              }
          : null
  const capabilities = resolveRuntimeCapabilities(
    snapshotCurrent,
    runtime,
    inventory,
  )
  const observed = capabilities.observedWorker
  const current = observed ?? capabilities.observedPane
  const { openChat, openTerminal } = useAgentWorkspace()
  const [ghosttyBusy, setGhosttyBusy] = useState(false)
  const [ghosttyFeedback, setGhosttyFeedback] = useState<{
    command?: string
    message: string
    state: 'error' | 'launched' | 'fallback'
  } | null>(null)

  if (!runtime || !terminalTarget || !terminalId || !runtimeSession) {
    return null
  }

  const contextLabel =
    project?.name ??
    (assignment
      ? projects?.find(
          (candidate) => candidate.id === assignment.project_id,
        )?.name
      : undefined) ??
    (coordinationNode
      ? coordinationNode.attached_project_ids
          .map(
            (projectId) =>
              projects?.find((candidate) => candidate.id === projectId)
                ?.name,
          )
          .filter((name): name is string => Boolean(name))
          .join(', ') || undefined
      : undefined) ??
    (target.kind === 'yard-orchestrator'
      ? 'Yard portfolio'
      : 'Yard workstream')
  const role =
    target.kind === 'yard-orchestrator'
      ? ('superintendent' as const)
      : target.kind === 'coordination-node'
        ? ('workstream' as const)
        : target.kind === 'orchestrator'
          ? ('orchestrator' as const)
          : ('worker' as const)
  const roleLabel =
    target.kind === 'yard-orchestrator'
      ? 'Superintendent'
      : target.kind === 'coordination-node'
        ? 'Workstream'
        : target.kind === 'orchestrator'
          ? 'Orchestrator'
          : assignment?.role ?? 'Worker'
  const workspaceTarget: Omit<AgentWorkspaceTarget, 'returnFocus'> = {
    capabilityReason: capabilities.reason,
    chatAvailable: capabilities.chat,
    contextLabel,
    cwd:
      current?.foreground_cwd ??
      current?.cwd ??
      coordinationNode?.cwd ??
      coordinationNode?.folder_path ??
      null,
    harness:
      observed?.display_provider ??
      current?.provider ??
      runtime.provider_session?.provider ??
      runtime.adapter,
    interactive: capabilities.terminal,
    key: targetKey,
    label,
    observation:
      capabilities.reason === 'stale'
        ? 'stale'
        : current
          ? 'observed'
          : 'durable',
    paneId: observed?.pane_id ?? current?.runtime_id ?? runtime.pane_id,
    role,
    roleLabel,
    runtimeAdapter: runtime.adapter,
    session: runtimeSession,
    status: runtimeCapabilityStatus(runtime, capabilities),
    tabId: current?.tab_id ?? runtime.tab_id,
    target,
    terminalId,
    terminalLeaseKey: terminalLeaseKey(target, runtime),
    terminalTarget,
    workspaceId: current?.workspace_id ?? runtime.workspace_id,
  }

  const attachCommand = ['herdr', '--session', runtimeSession, 'agent', 'attach', terminalId]
    .map(shellQuote)
    .join(' ')
  const capabilityLabel = runtimeCapabilityLabel(capabilities)
  const capabilityDetail = runtimeCapabilityDetail(capabilities)
  const lastKnownContext = [
    `session ${runtimeSession}`,
    `terminal ${terminalId}`,
    `pane ${observed?.pane_id ?? current?.runtime_id ?? runtime.pane_id}`,
    current?.foreground_cwd ?? current?.cwd ?? null,
  ]
    .filter(Boolean)
    .join(' · ')

  const openGhostty = async () => {
    if (!capabilities.terminal || ghosttyBusy) return
    setGhosttyBusy(true)
    setGhosttyFeedback(null)
    try {
      await openTerminalInGhostty(runtimeSession, terminalId)
      setGhosttyFeedback({
        message: 'Opened the live terminal in Ghostty.',
        state: 'launched',
      })
    } catch (caught) {
      if (
        caught instanceof YardApiError &&
        caught.code === 'ghostty_unavailable'
      ) {
        let copied = false
        try {
          await navigator.clipboard.writeText(attachCommand)
          copied = true
        } catch {
          // The validated command remains visible when clipboard access fails.
        }
        setGhosttyFeedback({
          command: copied ? undefined : attachCommand,
          message: copied
            ? 'Ghostty is not available on the Yard host. Attach command copied.'
            : caught.message,
          state: 'fallback',
        })
      } else {
        setGhosttyFeedback({
          message:
            caught instanceof Error
              ? caught.message
              : 'Ghostty could not be opened.',
          state: 'error',
        })
      }
    } finally {
      setGhosttyBusy(false)
    }
  }

  if (!capabilities.terminal && !capabilities.chat) {
    return (
      <section
        aria-label="Agent controls"
        className="intervention-section agent-controls"
      >
        <p className="eyebrow">Intervention</p>
        <div className="awaiting-disposition" role="status">
          <CircleAlert aria-hidden="true" size={16} />
          <span>
            <strong>Live controls unavailable</strong>
            <small>
              {capabilityLabel}. {capabilityDetail}
            </small>
          </span>
        </div>
        <div className="agent-control-launchers">
          <div className="agent-control-launcher">
            <span>
              <SquareTerminal aria-hidden="true" size={17} />
              <span>
                <strong>Last known runtime</strong>
                <small>{lastKnownContext}</small>
              </span>
            </span>
          </div>
        </div>
        <div className="inspector-actions">
          {onRefresh ? (
            <button
              className="secondary-button"
              onClick={onRefresh}
              type="button"
            >
              <RefreshCw aria-hidden="true" size={15} />
              Refresh inventory
            </button>
          ) : null}
        </div>
        <p className="project-orchestrator-transfer-status" role="status">
          Reattach becomes available after Yard sees one current matching terminal.
        </p>
      </section>
    )
  }

  return (
    <>
      <section
        aria-label="Agent controls"
        className="intervention-section agent-controls"
      >
        <p className="eyebrow">Intervention</p>
        <div className="agent-control-launchers">
          <div className="agent-control-launcher">
            <span>
              <SquareTerminal aria-hidden="true" size={17} />
              <span>
                <strong>Terminal</strong>
                <small>{terminalId}</small>
              </span>
            </span>
            <div className="agent-control-actions">
              <button
                className="secondary-button"
                onClick={(event) =>
                  openTerminal({
                    ...workspaceTarget,
                    returnFocus: event.currentTarget,
                  })
                }
                type="button"
              >
                <SquareTerminal aria-hidden="true" size={15} />
                Open terminal
              </button>
              <button
                className="icon-button"
                disabled={ghosttyBusy}
                onClick={() => void openGhostty()}
                title="Open this live terminal in Ghostty"
                type="button"
              >
                {ghosttyBusy ? (
                  <LoaderCircle
                    aria-label="Opening Ghostty"
                    className="status-spin"
                    size={16}
                  />
                ) : (
                  <ExternalLink aria-label="Open in Ghostty" size={16} />
                )}
              </button>
            </div>
          </div>
          {ghosttyFeedback ? (
            <div
              aria-live="polite"
              className="external-terminal-feedback"
              data-state={ghosttyFeedback.state}
            >
              <span>{ghosttyFeedback.message}</span>
              {ghosttyFeedback.command ? (
                <code>{ghosttyFeedback.command}</code>
              ) : null}
            </div>
          ) : null}
          <div className="agent-control-launcher">
            <span>
              <MessageSquareText aria-hidden="true" size={17} />
              <span>
                <strong>Agent chat</strong>
                <small>{runtimeCapabilityStatus(runtime, capabilities)} · {label}</small>
              </span>
            </span>
            {capabilities.chat ? (
              <button
                className="secondary-button"
                onClick={(event) =>
                  openChat({
                    ...workspaceTarget,
                    returnFocus: event.currentTarget,
                  })
                }
                type="button"
              >
                <MessageSquareText aria-hidden="true" size={15} />
                Open chat
              </button>
            ) : (
              <span className="project-orchestrator-transfer-status" role="status">
                {capabilityDetail}
              </span>
            )}
          </div>
        </div>
      </section>
    </>
  )
}

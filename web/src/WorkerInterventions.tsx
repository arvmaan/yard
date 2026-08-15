import { useState } from 'react'
import {
  ExternalLink,
  LoaderCircle,
  MessageSquareText,
  SquareTerminal,
} from 'lucide-react'
import {
  agentWorkspaceKey,
  useAgentWorkspace,
} from './AgentWorkspaceContext'
import { openTerminalInGhostty } from './api'
import type {
  Assignment,
  CoordinationNode,
  CoordinationNodeRoute,
  ObservedStatus,
  Project,
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
  projects,
  routes,
  status,
  target,
}: {
  onCoordinationChange?: (route: YardOrchestratorRoute) => void
  onCoordinationNodeChange?: (route: CoordinationNodeRoute) => void
  projects?: Project[]
  routes?: Array<YardOrchestratorRoute | CoordinationNodeRoute>
  status?: ObservedStatus
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
  const terminalId =
    assignment?.worker.runtime?.terminal_id ??
    project?.orchestrator.runtime?.terminal_id ??
    yardOrchestrator?.worker?.runtime?.terminal_id ??
    coordinationNode?.worker?.runtime?.terminal_id
  const runtimeSession =
    assignment?.worker.runtime?.session ??
    project?.orchestrator.runtime?.session ??
    yardOrchestrator?.worker?.runtime?.session ??
    coordinationNode?.worker?.runtime?.session
  const targetKey =
    agentWorkspaceKey(target)
  const interactive =
    Boolean(terminalId) &&
    (target.kind === 'orchestrator' ||
      target.kind === 'yard-orchestrator' ||
      target.kind === 'coordination-node' ||
      assignment?.lifecycle === 'active')
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
  const { openChat, openTerminal } = useAgentWorkspace()
  const [ghosttyBusy, setGhosttyBusy] = useState(false)
  const [ghosttyFeedback, setGhosttyFeedback] = useState<{
    command?: string
    message: string
    state: 'launched' | 'fallback'
  } | null>(null)
  if (!interactive) return null

  if (!terminalTarget || !terminalId || !runtimeSession) return null

  const workspaceTarget = {
    key: targetKey,
    label,
    projectName:
      project?.name ??
      (assignment
        ? projects?.find(
            (candidate) => candidate.id === assignment.project_id,
          )?.name
        : undefined) ??
      coordinationNode?.name ??
      'Yard portfolio',
    session: runtimeSession,
    status: status ?? 'unknown',
    target,
    terminalId,
    terminalTarget,
    workspaceId:
      assignment?.worker.runtime?.workspace_id ??
      project?.orchestrator.runtime?.workspace_id ??
      yardOrchestrator?.worker?.runtime?.workspace_id ??
      coordinationNode?.worker?.runtime?.workspace_id ??
      'workspace unavailable',
  }

  const attachCommand =
    runtimeSession && terminalId
      ? ['herdr', '--session', runtimeSession, 'agent', 'attach', terminalId]
          .map(shellQuote)
          .join(' ')
      : ''

  const openGhostty = async () => {
    if (!runtimeSession || !terminalId || ghosttyBusy) return
    setGhosttyBusy(true)
    setGhosttyFeedback(null)
    try {
      await openTerminalInGhostty(runtimeSession, terminalId)
      setGhosttyFeedback({
        message: 'Opened the live terminal in Ghostty.',
        state: 'launched',
      })
    } catch (caught) {
      let copied = false
      try {
        await navigator.clipboard.writeText(attachCommand)
        copied = true
      } catch {
        // The command remains visible when clipboard access is unavailable.
      }
      setGhosttyFeedback({
        command: copied ? undefined : attachCommand,
        message: copied
          ? 'Ghostty is not available on the Yard host. Attach command copied.'
          : caught instanceof Error
            ? caught.message
            : 'Ghostty could not be opened.',
        state: 'fallback',
      })
    } finally {
      setGhosttyBusy(false)
    }
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
              {terminalTarget ? (
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
              ) : null}
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
                <small>{status ?? 'unknown'} · {label}</small>
              </span>
            </span>
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
          </div>
        </div>
      </section>

    </>
  )
}

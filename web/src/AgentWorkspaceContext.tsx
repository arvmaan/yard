import { createContext, useContext } from 'react'
import type {
  AgentChatTarget,
} from './AgentChatWorkspace'
import type { TerminalTarget } from './TerminalSession'
import type { ObservedStatus, WorkerRuntimeBinding } from './types'
import type { AgentWindowRole } from './agentWindowNavigator'

export type TerminalPresentation = 'focus' | 'terminal'
export const DEFAULT_TERMINAL_PRESENTATION: TerminalPresentation = 'terminal'
export type AgentWorkspaceMode = 'changes' | 'chat' | 'terminal'
export type AgentWorkspaceView = AgentWorkspaceMode | 'map'

export function isTerminalWorkspaceMode(
  mode: AgentWorkspaceView,
): mode is 'terminal' {
  return mode === 'terminal'
}

export interface AgentWorkspaceTarget {
  contextLabel: string
  cwd: string | null
  harness: string
  key: string
  label: string
  observation: 'observed' | 'durable'
  paneId: string
  returnFocus?: HTMLElement | null
  role: AgentWindowRole
  roleLabel: string
  runtimeAdapter: string
  session: string
  status: ObservedStatus
  tabId: string | null
  target: AgentChatTarget
  terminalId: string
  terminalLeaseKey: string
  terminalTarget: TerminalTarget
  workspaceId: string
}

interface AgentWorkspaceContextValue {
  openChat: (target: AgentWorkspaceTarget) => void
  openTerminal: (target: AgentWorkspaceTarget) => void
}

export const AgentWorkspaceContext =
  createContext<AgentWorkspaceContextValue | null>(null)

export function useAgentWorkspace() {
  const context = useContext(AgentWorkspaceContext)
  if (!context) {
    throw new Error(
      'Agent workspace controls require an AgentWorkspaceContext provider',
    )
  }
  return context
}

export function agentWorkspaceKey(target: AgentChatTarget) {
  if (target.kind === 'assignment') {
    return `assignment:${target.assignment.id}`
  }
  if (target.kind === 'orchestrator') {
    return `orchestrator:${target.project.id}`
  }
  if (target.kind === 'coordination-node') {
    return `coordination-node:${target.node.id}`
  }
  return 'yard-orchestrator'
}

export function terminalLeaseKey(
  target: AgentChatTarget,
  runtime: WorkerRuntimeBinding,
) {
  const targetIdentity =
    target.kind === 'assignment'
      ? [
          target.kind,
          target.assignment.project_id,
          target.assignment.id,
          target.assignment.attempt.id,
          target.assignment.worker.id,
        ]
      : target.kind === 'orchestrator'
        ? [
            target.kind,
            target.project.id,
            target.project.orchestrator.id,
          ]
        : target.kind === 'coordination-node'
          ? [
              target.kind,
              target.node.id,
              target.node.version,
              target.node.worker?.id,
            ]
          : [target.kind, target.orchestrator.worker?.id]
  return JSON.stringify([
    ...targetIdentity,
    runtime.adapter,
    runtime.session,
    runtime.workspace_id,
    runtime.terminal_id,
    runtime.tab_id,
    runtime.pane_id,
    runtime.provider_session,
  ])
}

import { createContext, useContext } from 'react'
import type {
  AgentChatTarget,
} from './AgentChatWorkspace'
import type { TerminalTarget } from './TerminalSession'
import type { ObservedStatus } from './types'

export type AgentWorkspaceMode = 'chat' | 'terminal'

export interface AgentWorkspaceTarget {
  key: string
  label: string
  projectName: string
  returnFocus?: HTMLElement | null
  session: string
  status: ObservedStatus
  target: AgentChatTarget
  terminalId: string
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

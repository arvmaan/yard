import { describe, expect, it } from 'vitest'
import type { AgentChatTarget } from './AgentChatWorkspace'
import {
  agentQuestionKey,
  clearAgentQuestion,
  latestAgentQuestion,
  recordAgentQuestion,
} from './agentQuestions'
import type {
  Assignment,
  Project,
  WorkerRuntimeBinding,
} from './types'

function assignmentTarget({
  attemptId = 'attempt-1',
  providerSessionValue = 'provider-session-1',
  terminalId = 'terminal-1',
}: {
  attemptId?: string
  providerSessionValue?: string
  terminalId?: string
} = {}): AgentChatTarget {
  const runtime = {
    adapter: 'herdr',
    pane_id: 'pane-1',
    provider_session: {
      kind: 'conversation',
      provider: 'claude',
      source: 'herdr',
      value: providerSessionValue,
    },
    session: 'alpha',
    tab_id: 'tab-1',
    terminal_id: terminalId,
    workspace_id: 'workspace-1',
  } as unknown as WorkerRuntimeBinding
  return {
    assignment: {
      attempt: { id: attemptId },
      id: 'assignment-1',
      project_id: 'project-1',
      worker: {
        id: 'worker-1',
        runtime,
      },
    } as unknown as Assignment,
    kind: 'assignment',
  }
}

function orchestratorTarget({
  providerSessionValue = 'provider-session-1',
  terminalId = 'terminal-1',
}: {
  providerSessionValue?: string
  terminalId?: string
} = {}): AgentChatTarget {
  const assignment = assignmentTarget({
    providerSessionValue,
    terminalId,
  })
  if (assignment.kind !== 'assignment') {
    throw new Error('Assignment fixture is invalid')
  }
  return {
    kind: 'orchestrator',
    project: {
      id: 'project-1',
      orchestrator: assignment.assignment.worker,
    } as unknown as Project,
  }
}

describe('agent questions', () => {
  it('scopes question state to the active runtime incarnation', () => {
    const current = agentQuestionKey(assignmentTarget())

    expect(
      agentQuestionKey(assignmentTarget({ attemptId: 'attempt-2' })),
    ).not.toBe(current)
    expect(
      agentQuestionKey(assignmentTarget({ terminalId: 'terminal-2' })),
    ).not.toBe(current)
    expect(
      agentQuestionKey(
        assignmentTarget({ providerSessionValue: 'provider-session-2' }),
      ),
    ).not.toBe(current)
    expect(agentQuestionKey(assignmentTarget())).toBe(current)
  })

  it('scopes project orchestrator state to the active runtime incarnation', () => {
    const current = agentQuestionKey(orchestratorTarget())

    expect(
      agentQuestionKey(
        orchestratorTarget({ terminalId: 'terminal-2' }),
      ),
    ).not.toBe(current)
    expect(
      agentQuestionKey(
        orchestratorTarget({
          providerSessionValue: 'provider-session-2',
        }),
      ),
    ).not.toBe(current)
    expect(agentQuestionKey(orchestratorTarget())).toBe(current)
  })

  it('clears only the expected recorded question', () => {
    const targetKey = agentQuestionKey(assignmentTarget())
    const question = {
      askedAtUnixMs: 1,
      id: 'question-1',
      text: 'What is the current status?',
    }
    recordAgentQuestion([targetKey], question)

    expect(clearAgentQuestion(targetKey, 'question-2')).toBe(false)
    expect(latestAgentQuestion(targetKey)).toEqual(question)
    expect(clearAgentQuestion(targetKey, question.id)).toBe(true)
    expect(latestAgentQuestion(targetKey)).toBeNull()
  })
})

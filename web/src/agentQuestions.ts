import { useSyncExternalStore } from 'react'
import type { AgentChatTarget } from './AgentChatWorkspace'
import type { TerminalTarget } from './TerminalSession'
import type { WorkerRuntimeBinding } from './types'

export interface AgentQuestion {
  askedAtUnixMs: number
  id: string
  text: string
}

const questions = new Map<string, AgentQuestion>()
const listeners = new Set<() => void>()
const MAX_RECORDED_QUESTIONS = 256

function workerQuestionIdentity(worker: {
  id: string
  runtime: WorkerRuntimeBinding | null
}) {
  const runtime = worker.runtime
  return runtime
    ? [
        worker.id,
        runtime.adapter,
        runtime.session,
        runtime.workspace_id,
        runtime.terminal_id,
        runtime.tab_id,
        runtime.pane_id,
        runtime.provider_session,
      ]
    : [worker.id, 'unbound']
}

function pruneQuestions() {
  if (questions.size <= MAX_RECORDED_QUESTIONS) return
  const oldest = [...questions.entries()].sort(
    ([, left], [, right]) =>
      left.askedAtUnixMs - right.askedAtUnixMs,
  )
  for (const [targetKey] of oldest.slice(
    0,
    questions.size - MAX_RECORDED_QUESTIONS,
  )) {
    questions.delete(targetKey)
  }
}

function emitChange() {
  for (const listener of listeners) listener()
}

export function agentQuestionKey(target: AgentChatTarget) {
  if (target.kind === 'assignment') {
    return JSON.stringify([
      'assignment',
      target.assignment.project_id,
      target.assignment.id,
      target.assignment.attempt.id,
      ...workerQuestionIdentity(target.assignment.worker),
    ])
  }
  if (target.kind === 'orchestrator') {
    return JSON.stringify([
      'orchestrator',
      target.project.id,
      ...workerQuestionIdentity(target.project.orchestrator),
    ])
  }
  if (target.kind === 'coordination-node') {
    return JSON.stringify([
      'coordination-node',
      target.node.id,
      ...(target.node.worker
        ? workerQuestionIdentity(target.node.worker)
        : ['unbound']),
    ])
  }
  return JSON.stringify([
    'yard-orchestrator',
    ...(target.orchestrator.worker
      ? workerQuestionIdentity(target.orchestrator.worker)
      : ['unbound']),
  ])
}

export function agentQuestionsMatch(left: string, right: string) {
  return (
    left.trim().replace(/\s+/g, ' ') ===
    right.trim().replace(/\s+/g, ' ')
  )
}

export function recordAgentQuestion(
  targetKeys: readonly string[],
  question: AgentQuestion,
) {
  for (const targetKey of new Set(targetKeys)) {
    questions.set(targetKey, question)
  }
  pruneQuestions()
  emitChange()
}

export function clearAgentQuestion(
  targetKey: string,
  questionId?: string,
) {
  const current = questions.get(targetKey)
  if (!current || (questionId && current.id !== questionId)) return false
  questions.delete(targetKey)
  emitChange()
  return true
}

export function latestAgentQuestion(targetKey: string) {
  return questions.get(targetKey) ?? null
}

export function terminalQuestionKey(target: TerminalTarget) {
  if (target.kind === 'assignment') {
    return `assignment:${target.assignmentId}`
  }
  if (target.kind === 'orchestrator') {
    return `orchestrator:${target.projectId}`
  }
  if (target.kind === 'coordination-node') {
    return `coordination-node:${target.nodeId}`
  }
  return 'yard-orchestrator'
}

export function useLatestAgentQuestion(targetKey: string) {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    () => latestAgentQuestion(targetKey),
    () => null,
  )
}

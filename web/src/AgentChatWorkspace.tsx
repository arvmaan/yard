import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent as ReactKeyboardEvent,
} from 'react'
import {
  Bot,
  CircleAlert,
  CircleCheck,
  LoaderCircle,
  MessageSquareText,
  RefreshCw,
  Send,
  X,
} from 'lucide-react'
import {
  fetchAssignmentTerminalOutput,
  fetchCoordinationNodeTerminalOutput,
  fetchOrchestratorTerminalOutput,
  fetchYardOrchestratorTerminalOutput,
  sendAssignmentPrompt,
  sendCoordinationNodePrompt,
  sendCoordinationNodeRoute,
  sendOrchestratorPrompt,
  sendYardOrchestratorPrompt,
  sendYardOrchestratorRoute,
} from './api'
import {
  agentQuestionKey,
  agentQuestionsMatch,
  recordAgentQuestion,
  type AgentQuestion,
  useLatestAgentQuestion,
} from './agentQuestions'
import {
  buildExecutionOrder,
  ORDER_TEMPLATES,
} from './orderTemplates'
import {
  CollapsibleAgentOutput,
  CollapsibleQuestion,
} from './CollapsibleAgentOutput'
import { promptRequiresNewCommand } from './promptPolicy'
import {
  normalizeTerminalOutput,
  parseTerminalTranscript,
  TERMINAL_OUTPUT_LINES,
  TRUNCATED_TERMINAL_OUTPUT_LABEL,
} from './terminalOutput'
import { useModalDialog } from './useModalDialog'
import type {
  Assignment,
  CoordinationNode,
  CoordinationNodeRoute,
  ObservedStatus,
  Project,
  SendAssignmentPromptInput,
  SendCoordinationNodePromptInput,
  SendCoordinationNodeRouteInput,
  SendOrchestratorPromptInput,
  SendYardOrchestratorPromptInput,
  SendYardOrchestratorRouteInput,
  YardOrchestrator,
  YardOrchestratorRoute,
} from './types'

export type AgentChatTarget =
  | { kind: 'assignment'; assignment: Assignment }
  | { kind: 'orchestrator'; project: Project }
  | { kind: 'yard-orchestrator'; orchestrator: YardOrchestrator }
  | { kind: 'coordination-node'; node: CoordinationNode }

type PromptFeedback =
  | { kind: 'success'; message: string }
  | {
      kind: 'error'
      message: string
      requiresNewCommand: boolean
    }
  | null

type RetainedPrompt =
  | {
      kind: 'assignment'
      targetKey: string
      text: string
      command: SendAssignmentPromptInput
    }
  | {
      kind: 'orchestrator'
      targetKey: string
      text: string
      command: SendOrchestratorPromptInput
    }
  | {
      kind: 'yard-orchestrator'
      targetKey: string
      text: string
      command: SendYardOrchestratorPromptInput
    }
  | {
      kind: 'yard-route'
      targetKey: string
      text: string
      command: SendYardOrchestratorRouteInput
    }
  | {
      kind: 'coordination-node'
      targetKey: string
      text: string
      command: SendCoordinationNodePromptInput
    }
  | {
      kind: 'coordination-route'
      targetKey: string
      text: string
      command: SendCoordinationNodeRouteInput
    }

interface ChatMessage {
  id: string
  kind: 'agent' | 'user' | 'error'
  label: string
  meta: string
  question?: AgentQuestion
  text: string
  truncated?: boolean
}

function errorMessage(caught: unknown, fallback: string) {
  return caught instanceof Error ? caught.message : fallback
}

function activitySegments(text: string) {
  const normalized = normalizeTerminalOutput(text)
  return normalized ? [normalized] : []
}

export function AgentChatWorkspace({
  label,
  onCoordinationChange,
  onCoordinationNodeChange,
  onClose,
  open,
  projects = [],
  returnFocus,
  routes = [],
  status = 'unknown',
  target,
  variant = 'modal',
}: {
  label: string
  onCoordinationChange?: (route: YardOrchestratorRoute) => void
  onCoordinationNodeChange?: (route: CoordinationNodeRoute) => void
  onClose: () => void
  open: boolean
  projects?: Project[]
  returnFocus: HTMLElement | null
  routes?: Array<YardOrchestratorRoute | CoordinationNodeRoute>
  status?: ObservedStatus
  target: AgentChatTarget
  variant?: 'modal' | 'workspace'
}) {
  const titleId = useId()
  const messageId = useId()
  const templateId = useId()
  const dispatchId = useId()
  const assignment =
    target.kind === 'assignment' ? target.assignment : null
  const project = target.kind === 'orchestrator' ? target.project : null
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
  const targetKey =
    target.kind === 'assignment'
      ? `assignment:${assignmentId}:${assignment?.attempt.id}`
      : target.kind === 'orchestrator'
        ? `orchestrator:${projectId}:${workerId}`
        : target.kind === 'yard-orchestrator'
          ? `yard-orchestrator:${yardOrchestrator?.version}:${workerId}`
          : `coordination-node:${coordinationNode?.id}:${coordinationNode?.version}:${workerId}`
  const workspaceQuestionKey = agentQuestionKey(target)
  const recordedQuestion = useLatestAgentQuestion(workspaceQuestionKey)
  const targetRole =
    target.kind === 'yard-orchestrator'
      ? 'Superintendent'
      : target.kind === 'coordination-node'
        ? 'workstream orchestrator'
      : target.kind === 'orchestrator'
        ? 'orchestrator'
        : 'worker'
  const contextLabel =
    target.kind === 'orchestrator'
      ? project?.name ?? 'Project'
      : target.kind === 'yard-orchestrator'
        ? 'Yard portfolio'
        : target.kind === 'coordination-node'
          ? coordinationNode?.name ?? 'Workstream'
        : assignment?.role ?? 'Assigned worker'
  const [messages, setMessages] = useState<ChatMessage[]>([])
  const [activityLoading, setActivityLoading] = useState(false)
  const [promptText, setPromptText] = useState('')
  const [promptBusy, setPromptBusy] = useState(false)
  const [dispatchProjectId, setDispatchProjectId] = useState('')
  const [promptFeedback, setPromptFeedback] =
    useState<PromptFeedback>(null)
  const activityMessages = messages.filter(
    (message) => message.kind === 'agent',
  )
  const transcriptQuestions = activityMessages.flatMap((message) =>
    parseTerminalTranscript(
      message.text,
      status,
      message.truncated,
    ).turns.flatMap((turn) => turn.question ? [turn.question] : []),
  )
  const recordedQuestionMessage =
    recordedQuestion &&
    !messages.some(
      (message) => message.question?.id === recordedQuestion.id,
    )
      ? {
          id: `question:${recordedQuestion.id}`,
          kind: 'user' as const,
          label: 'Your question',
          meta: 'Sent from another Yard control',
          question: recordedQuestion,
          text: recordedQuestion.text,
        }
      : null
  const questionMessages = [
    ...(recordedQuestionMessage ? [recordedQuestionMessage] : []),
    ...messages.filter((message) => message.kind === 'user'),
  ]
  const pendingQuestions = questionMessages.filter(
    (message) =>
      !transcriptQuestions.some((question) =>
        agentQuestionsMatch(question, message.text),
      ),
  )
  const threadMessages = [
    ...activityMessages,
    ...pendingQuestions,
    ...messages.filter(
      (message) =>
        message.kind === 'error' &&
        message.id.startsWith('activity:error:'),
    ),
  ]
  const outputController = useRef<AbortController | null>(null)
  const promptInFlight = useRef(false)
  const targetKeyRef = useRef(targetKey)
  const retainedPromptCommand = useRef<RetainedPrompt | null>(null)
  const onCloseRef = useRef(onClose)
  const returnFocusRef = useRef(returnFocus)
  const dialogRef = useRef<HTMLElement>(null)
  const messagesRef = useRef<HTMLDivElement>(null)
  const followMessagesRef = useRef(true)
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const refreshTimer = useRef<number | null>(null)
  const dispatchProject =
    target.kind === 'yard-orchestrator' ||
    target.kind === 'coordination-node'
      ? projects.find((candidate) => candidate.id === dispatchProjectId) ??
        null
      : null
  const promptTargetKey = dispatchProject
    ? target.kind === 'coordination-node'
      ? `coordination-route:${coordinationNode?.id}:${dispatchProject.id}:${dispatchProject.orchestrator.id}`
      : `yard-route:${dispatchProject.id}:${dispatchProject.orchestrator.id}`
    : targetKey
  const activeContextLabel = dispatchProject?.name ?? contextLabel
  onCloseRef.current = onClose
  returnFocusRef.current = returnFocus
  useModalDialog({
    active: open && variant === 'modal',
    dialogRef,
    initialFocusRef: textareaRef,
    onClose,
    returnFocus,
  })

  useLayoutEffect(() => {
    targetKeyRef.current = targetKey
  }, [targetKey])

  const loadActivity = useCallback(async () => {
    outputController.current?.abort()
    const controller = new AbortController()
    outputController.current = controller
    setActivityLoading(true)

    try {
      const output =
        target.kind === 'assignment' && assignmentId
          ? await fetchAssignmentTerminalOutput(
              projectId,
              assignmentId,
              TERMINAL_OUTPUT_LINES,
              controller.signal,
            )
          : target.kind === 'orchestrator'
            ? await fetchOrchestratorTerminalOutput(
                projectId,
                TERMINAL_OUTPUT_LINES,
                controller.signal,
              )
            : target.kind === 'coordination-node' && coordinationNode
              ? await fetchCoordinationNodeTerminalOutput(
                  coordinationNode.id,
                  TERMINAL_OUTPUT_LINES,
                  controller.signal,
                )
            : await fetchYardOrchestratorTerminalOutput(
                TERMINAL_OUTPUT_LINES,
                controller.signal,
              )
      if (controller.signal.aborted) return
      const segments = output.text
        ? activitySegments(output.text)
        : []
      const activity: ChatMessage[] = (
        segments.length > 0 ? segments : ['No recent agent output.']
      ).map((text, index, allSegments) => ({
        id: `activity:${targetKey}:${output.revision}:${index}`,
        kind: 'agent',
        label,
        meta:
          allSegments.length > 1
            ? `Agent output ${index + 1}/${allSegments.length} · rev ${output.revision}`
            : `Agent output${
                output.truncated
                  ? ` · ${TRUNCATED_TERMINAL_OUTPUT_LABEL}`
                  : ''
              } · rev ${output.revision}`,
        text,
        truncated: output.truncated,
      }))
      setMessages((current) => [
        ...current.filter(
          (message) => !message.id.startsWith('activity:'),
        ),
        ...activity,
      ])
    } catch (caught) {
      if (
        controller.signal.aborted ||
        (caught instanceof DOMException && caught.name === 'AbortError')
      ) {
        return
      }
      setMessages((current) => [
        ...current.filter(
          (message) => !message.id.startsWith('activity:'),
        ),
        {
          id: `activity:error:${targetKey}`,
          kind: 'error',
          label: 'Agent output unavailable',
          meta: contextLabel,
          text: errorMessage(caught, 'Recent agent output is unavailable'),
        },
      ])
    } finally {
      if (
        !controller.signal.aborted &&
        outputController.current === controller
      ) {
        setActivityLoading(false)
      }
    }
  }, [
    assignmentId,
    coordinationNode,
    contextLabel,
    label,
    projectId,
    target.kind,
    targetKey,
  ])

  useEffect(() => {
    outputController.current?.abort()
    outputController.current = null
    promptInFlight.current = false
    retainedPromptCommand.current = null
    setMessages([])
    setPromptText('')
    setPromptFeedback(null)
    setPromptBusy(false)
    setDispatchProjectId('')
    followMessagesRef.current = true
    return () => outputController.current?.abort()
  }, [targetKey])

  useEffect(() => {
    if (!open) return
    followMessagesRef.current = true
    void loadActivity()
    window.requestAnimationFrame(() => textareaRef.current?.focus())

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onCloseRef.current()
    }
    if (variant === 'workspace') {
      window.addEventListener('keydown', handleKeyDown)
    }
    return () => {
      window.removeEventListener('keydown', handleKeyDown)
      outputController.current?.abort()
      if (refreshTimer.current !== null) {
        window.clearTimeout(refreshTimer.current)
        refreshTimer.current = null
      }
      if (variant === 'workspace') {
        window.setTimeout(() => returnFocusRef.current?.focus(), 0)
      }
    }
  }, [loadActivity, open, variant])

  useEffect(() => {
    if (!open) return
    const messagesElement = messagesRef.current
    if (messagesElement && followMessagesRef.current) {
      messagesElement.scrollTop = messagesElement.scrollHeight
    }
  }, [messages, open])

  const handleMessagesScroll = () => {
    const messagesElement = messagesRef.current
    if (!messagesElement) return
    followMessagesRef.current =
      messagesElement.scrollHeight -
        messagesElement.scrollTop -
        messagesElement.clientHeight <=
      48
  }

  const sendPrompt = async (forceNewCommand: boolean) => {
    const text = promptText.trim()
    if (!text || promptInFlight.current) return
    const requestedTargetKey = targetKeyRef.current

    const retainedCommand = retainedPromptCommand.current
    const canReuse =
      !forceNewCommand &&
      retainedCommand?.targetKey === promptTargetKey &&
      retainedCommand.text === text
    const retained = canReuse
      ? retainedCommand
      : target.kind === 'assignment' && assignment
        ? {
            kind: 'assignment' as const,
            targetKey: promptTargetKey,
            text,
            command: {
              command_id: crypto.randomUUID(),
              actor: 'local-user',
              attempt_id: assignment.attempt.id,
              expected_assignment_version: assignment.version,
              expected_attempt_version: assignment.attempt.version,
              text: buildExecutionOrder(text),
            },
          }
        : target.kind === 'orchestrator'
          ? {
            kind: 'orchestrator' as const,
            targetKey: promptTargetKey,
            text,
            command: {
              command_id: crypto.randomUUID(),
              actor: 'local-user',
              expected_project_version: project?.version ?? '0',
              orchestrator_worker_id: workerId,
              text: buildExecutionOrder(text),
            },
          }
          : target.kind === 'coordination-node' && coordinationNode
            ? dispatchProject
              ? {
                  kind: 'coordination-route' as const,
                  targetKey: promptTargetKey,
                  text,
                  command: {
                    command_id: crypto.randomUUID(),
                    actor: 'local-user',
                    expected_node_version: coordinationNode.version,
                    worker_id: workerId,
                    target_project_id: dispatchProject.id,
                    expected_project_version: dispatchProject.version,
                    target_orchestrator_worker_id:
                      dispatchProject.orchestrator.id,
                    text: buildExecutionOrder(text),
                  },
                }
              : {
                  kind: 'coordination-node' as const,
                  targetKey: promptTargetKey,
                  text,
                  command: {
                    command_id: crypto.randomUUID(),
                    actor: 'local-user',
                    expected_node_version: coordinationNode.version,
                    worker_id: workerId,
                    text: buildExecutionOrder(text),
                  },
                }
          : dispatchProject
            ? {
                kind: 'yard-route' as const,
                targetKey: promptTargetKey,
                text,
                command: {
                  command_id: crypto.randomUUID(),
                  actor: 'local-user',
                  expected_orchestrator_version:
                    yardOrchestrator?.version ?? '0',
                  orchestrator_worker_id: workerId,
                  target_project_id: dispatchProject.id,
                  expected_project_version: dispatchProject.version,
                  target_orchestrator_worker_id:
                    dispatchProject.orchestrator.id,
                  text: buildExecutionOrder(text),
                },
              }
            : {
              kind: 'yard-orchestrator' as const,
              targetKey: promptTargetKey,
              text,
              command: {
                command_id: crypto.randomUUID(),
                actor: 'local-user',
                expected_orchestrator_version:
                  yardOrchestrator?.version ?? '0',
                orchestrator_worker_id: workerId,
                text: buildExecutionOrder(text),
              },
            }
    if (!retained) return

    retainedPromptCommand.current = retained
    if (!canReuse) {
      const question = {
        askedAtUnixMs: Date.now(),
        id: retained.command.command_id,
        text,
      }
      recordAgentQuestion(
        [
          workspaceQuestionKey,
          ...(dispatchProject
            ? [
                agentQuestionKey({
                  kind: 'orchestrator',
                  project: dispatchProject,
                }),
              ]
            : []),
        ],
        question,
      )
      setMessages((current) => [
        ...current,
        {
          id: `user:${retained.command.command_id}`,
          kind: 'user',
          label: 'Your question',
          meta: forceNewCommand ? 'New command' : activeContextLabel,
          question,
          text,
        },
      ])
    }
    promptInFlight.current = true
    setPromptBusy(true)
    setPromptFeedback(null)

    try {
      if (retained.kind === 'assignment') {
        await sendAssignmentPrompt(
          projectId,
          assignmentId ?? '',
          retained.command,
        )
      } else if (retained.kind === 'orchestrator') {
        await sendOrchestratorPrompt(projectId, retained.command)
      } else if (retained.kind === 'yard-route') {
        const route = await sendYardOrchestratorRoute(retained.command)
        onCoordinationChange?.(route)
      } else if (retained.kind === 'coordination-route') {
        const route = await sendCoordinationNodeRoute(
          coordinationNode?.id ?? '',
          retained.command,
        )
        onCoordinationNodeChange?.(route)
      } else if (retained.kind === 'coordination-node') {
        await sendCoordinationNodePrompt(
          coordinationNode?.id ?? '',
          retained.command,
        )
      } else {
        await sendYardOrchestratorPrompt(retained.command)
      }
      // Prompt POSTs cannot be aborted after submission. Ignore their UI
      // result if the operator switched chat targets while one was in flight.
      if (targetKeyRef.current !== requestedTargetKey) return
      setPromptText('')
      retainedPromptCommand.current = null
      setPromptFeedback({
        kind: 'success',
        message:
          retained.kind === 'yard-route' ||
          retained.kind === 'coordination-route'
            ? `${target.kind === 'coordination-node' ? 'Workstream' : 'Yard'} delivered this order to ${activeContextLabel}.`
            : 'Order delivered to the agent.',
      })
      refreshTimer.current = window.setTimeout(() => {
        refreshTimer.current = null
        void loadActivity()
      }, 800)
    } catch (caught) {
      if (targetKeyRef.current !== requestedTargetKey) return
      const feedback = {
        kind: 'error' as const,
        message: errorMessage(caught, 'Prompt acknowledgement failed'),
        requiresNewCommand: promptRequiresNewCommand(caught),
      }
      setPromptFeedback(feedback)
      setMessages((current) => [
        ...current.filter(
          (message) =>
            message.id !== `delivery:${retained.command.command_id}`,
        ),
        {
          id: `delivery:${retained.command.command_id}`,
          kind: 'error',
          label: 'Delivery uncertain',
          meta: targetRole,
          text: feedback.message,
        },
      ])
    } finally {
      if (targetKeyRef.current === requestedTargetKey) {
        promptInFlight.current = false
        setPromptBusy(false)
      }
    }
  }

  const submitPrompt = async (event: FormEvent) => {
    event.preventDefault()
    await sendPrompt(false)
  }

  const updatePrompt = (nextText: string) => {
    setPromptText(nextText)
    if (
      promptFeedback?.kind === 'error' ||
      (retainedPromptCommand.current &&
        retainedPromptCommand.current.text !== nextText.trim())
    ) {
      retainedPromptCommand.current = null
    }
    if (promptFeedback) setPromptFeedback(null)
  }

  const handleComposerKeyDown = (
    event: ReactKeyboardEvent<HTMLTextAreaElement>,
  ) => {
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
      event.preventDefault()
      event.currentTarget.form?.requestSubmit()
    }
  }

  if (!open) return null

  const workspace = (
    <section
      aria-label={
        variant === 'workspace' ? `${label} agent chat` : undefined
      }
      aria-labelledby={variant === 'modal' ? titleId : undefined}
      aria-modal={variant === 'modal' ? true : undefined}
      className={`chat-workspace chat-workspace--${variant}`}
      ref={dialogRef}
      role={variant === 'modal' ? 'dialog' : 'region'}
    >
      {variant === 'modal' ? (
        <header className="chat-workspace__header">
          <span className="chat-workspace__icon">
            <MessageSquareText aria-hidden="true" size={20} />
          </span>
          <div>
            <p className="eyebrow">Agent chat</p>
            <h2 id={titleId}>{label}</h2>
          </div>
          <span className="status-badge" data-status={status}>
            <Bot aria-hidden="true" size={13} />
            {status}
          </span>
          <button
            aria-label="Close chat"
            className="icon-button"
            onClick={onClose}
            title="Close chat"
            type="button"
          >
            <X aria-hidden="true" size={18} />
          </button>
        </header>
      ) : null}

      <div className="chat-workspace__body">
          <aside className="chat-context">
            <div>
              <p className="eyebrow">Context</p>
                  <strong>{contextLabel}</strong>
                  <small>{targetRole}</small>
                </div>
            {target.kind === 'yard-orchestrator' ||
            target.kind === 'coordination-node' ? (
              <>
                <label className="field-label" htmlFor={dispatchId}>
                  Dispatch scope
                </label>
                <select
                  id={dispatchId}
                  onChange={(event) => {
                    setDispatchProjectId(event.target.value)
                    retainedPromptCommand.current = null
                    setPromptFeedback(null)
                  }}
                  value={dispatchProjectId}
                >
                  <option value="">
                    {target.kind === 'coordination-node'
                      ? 'Workstream orchestrator'
                      : 'Superintendent'}
                  </option>
                  {projects.map((candidate) => (
                    <option key={candidate.id} value={candidate.id}>
                      {candidate.name}
                    </option>
                  ))}
                </select>
                {routes.length > 0 ? (
                  <div className="chat-route-statuses">
                    <p className="eyebrow">Recent routes</p>
                    {routes.slice(0, 5).map((route) => {
                      const routedProject = projects.find(
                        (candidate) =>
                          candidate.id === route.target_project_id,
                      )
                      return (
                        <span
                          data-status={route.status}
                          key={route.command_id}
                        >
                          <strong>
                            {routedProject?.name ??
                              route.target_project_id}
                          </strong>
                          <small>{route.status}</small>
                        </span>
                      )
                    })}
                  </div>
                ) : null}
              </>
            ) : null}
            {status === 'blocked' || status === 'unknown' ? (
              <div className="chat-intervention-state" role="status">
                <CircleAlert aria-hidden="true" size={16} />
                <span>
                  <strong>Manual intervention</strong>
                  <small>
                    Send the smallest decision or input needed to unblock this
                    {` ${targetRole}`}.
                  </small>
                </span>
              </div>
            ) : null}
            <label className="field-label" htmlFor={templateId}>
              Order template
            </label>
            <select
              id={templateId}
              onChange={(event) => {
                const template = ORDER_TEMPLATES.find(
                  ({ id }) => id === event.target.value,
                )
                if (template) updatePrompt(template.text)
              }}
              value=""
            >
              <option value="">Choose an order...</option>
              {ORDER_TEMPLATES.map((template) => (
                <option key={template.id} value={template.id}>
                  {template.label}
                </option>
              ))}
            </select>
          </aside>

          <section
            aria-label="Agent conversation"
            className="chat-thread"
          >
            <header className="chat-thread__header">
              <div>
                <p className="eyebrow">Transcript</p>
                <h3>Questions and answers</h3>
              </div>
              <button
                aria-label="Refresh agent activity"
                className="icon-button"
                disabled={activityLoading}
                onClick={() => void loadActivity()}
                title="Refresh agent activity"
                type="button"
              >
                <RefreshCw
                  aria-hidden="true"
                  className={activityLoading ? 'status-spin' : ''}
                  size={16}
                />
              </button>
            </header>
            <div
              aria-live="polite"
              className="chat-thread__messages"
              onScroll={handleMessagesScroll}
              ref={messagesRef}
            >
              {activityLoading && messages.length === 0 ? (
                <div className="chat-thread__loading" role="status">
                  <LoaderCircle
                    aria-hidden="true"
                    className="status-spin"
                    size={18}
                  />
                  Loading recent agent activity
                </div>
              ) : null}
              {threadMessages.map((message) => (
                <article
                  className="chat-message"
                  data-kind={message.kind}
                  data-question-id={message.question?.id}
                  key={message.id}
                >
                  {message.kind === 'agent' ? (
                    <CollapsibleAgentOutput
                      status={status}
                      text={message.text}
                      truncated={message.truncated}
                    />
                  ) : message.kind === 'user' ? (
                    <CollapsibleQuestion
                      meta={message.meta}
                      text={message.text}
                    />
                  ) : (
                    <>
                      <header>
                        <strong>{message.label}</strong>
                        <small>{message.meta}</small>
                      </header>
                      <p>{message.text}</p>
                    </>
                  )}
                </article>
              ))}
            </div>
          </section>
      </div>

      <form
        className="chat-composer"
        onSubmit={(event) => void submitPrompt(event)}
        >
          <label className="field-label" htmlFor={messageId}>
            Message
          </label>
          <div className="chat-composer__input">
            <textarea
              id={messageId}
              maxLength={16000}
              onChange={(event) => updatePrompt(event.target.value)}
              onKeyDown={handleComposerKeyDown}
              placeholder="Give this agent its next order..."
              ref={textareaRef}
              rows={3}
              value={promptText}
            />
            <button
              aria-label="Send order"
              className="command-button"
              disabled={
                promptBusy ||
                !promptText.trim() ||
                (promptFeedback?.kind === 'error' &&
                  promptFeedback.requiresNewCommand)
              }
              title="Send order"
              type="submit"
            >
              {promptBusy ? (
                <LoaderCircle
                  aria-hidden="true"
                  className="status-spin"
                  size={17}
                />
              ) : (
                <Send aria-hidden="true" size={17} />
              )}
            </button>
          </div>
          {promptFeedback ? (
            <div
              className="chat-delivery-feedback"
              data-kind={promptFeedback.kind}
              role={promptFeedback.kind === 'error' ? 'alert' : 'status'}
            >
              {promptFeedback.kind === 'success' ? (
                <>
                  <CircleCheck aria-hidden="true" size={15} />
                  <span>{promptFeedback.message}</span>
                </>
              ) : (
                <>
                  <CircleAlert aria-hidden="true" size={15} />
                  <span>
                    <strong>Herdr acknowledgement unavailable.</strong>
                    <small>{promptFeedback.message}</small>
                    {promptFeedback.requiresNewCommand ? (
                      <>
                        <small>
                          This command ID cannot be retried safely. Send it as a
                          new command only if another delivery is intended.
                        </small>
                        <button
                          className="secondary-button"
                          disabled={promptBusy}
                          onClick={() => void sendPrompt(true)}
                          type="button"
                        >
                          <Send aria-hidden="true" size={15} />
                          Send as new command
                        </button>
                      </>
                    ) : null}
                  </span>
                </>
              )}
            </div>
          ) : null}
      </form>
    </section>
  )

  return variant === 'workspace' ? (
    workspace
  ) : (
    <div className="modal-backdrop chat-backdrop" role="presentation">
      {workspace}
    </div>
  )
}

import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent as ReactKeyboardEvent,
} from 'react'
import { createPortal } from 'react-dom'
import {
  Bot,
  CircleAlert,
  CircleCheck,
  LoaderCircle,
  MessageSquareText,
  RefreshCw,
  Send,
  Users,
  X,
} from 'lucide-react'
import {
  fetchAssignmentTerminalOutput,
  fetchOrchestratorTerminalOutput,
  sendAssignmentPrompt,
  sendOrchestratorPrompt,
} from './api'
import {
  agentQuestionKey,
  recordAgentQuestion,
  type AgentQuestion,
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
  TERMINAL_OUTPUT_LINES,
  TRUNCATED_TERMINAL_OUTPUT_LABEL,
} from './terminalOutput'
import { useModalDialog } from './useModalDialog'
import type {
  Assignment,
  ObservedStatus,
  Project,
  SendAssignmentPromptInput,
  SendOrchestratorPromptInput,
} from './types'

export type AgentGroupTarget =
  | {
      assignment: Assignment
      kind: 'assignment'
      label: string
      projectName: string
      status: ObservedStatus
    }
  | {
      kind: 'orchestrator'
      label: string
      project: Project
      status: ObservedStatus
    }

type TargetCommand =
  | {
      command: SendAssignmentPromptInput
      kind: 'assignment'
      target: Extract<AgentGroupTarget, { kind: 'assignment' }>
    }
  | {
      command: SendOrchestratorPromptInput
      kind: 'orchestrator'
      target: Extract<AgentGroupTarget, { kind: 'orchestrator' }>
    }

interface TargetDelivery {
  error?: string
  requiresNewCommand?: boolean
  state: 'pending' | 'delivered' | 'failed'
}

interface ThreadMessage {
  id: string
  kind: 'agent' | 'user' | 'error'
  label: string
  meta: string
  question?: AgentQuestion
  status?: ObservedStatus
  text: string
  truncated?: boolean
}

function agentGroupTargetKey(target: AgentGroupTarget) {
  return agentQuestionKey(
    target.kind === 'assignment'
      ? {
          assignment: target.assignment,
          kind: 'assignment',
        }
      : {
          kind: 'orchestrator',
          project: target.project,
        },
  )
}

function targetProjectName(target: AgentGroupTarget) {
  return target.kind === 'assignment'
    ? target.projectName
    : target.project.name
}

function errorMessage(caught: unknown, fallback: string) {
  return caught instanceof Error ? caught.message : fallback
}

function commandForTarget(
  target: AgentGroupTarget,
  text: string,
): TargetCommand {
  if (target.kind === 'assignment') {
    return {
      kind: 'assignment',
      target,
      command: {
        command_id: crypto.randomUUID(),
        actor: 'local-user',
        attempt_id: target.assignment.attempt.id,
        expected_assignment_version: target.assignment.version,
        expected_attempt_version: target.assignment.attempt.version,
        text: buildExecutionOrder(text),
      },
    }
  }
  return {
    kind: 'orchestrator',
    target,
    command: {
      command_id: crypto.randomUUID(),
      actor: 'local-user',
      expected_project_version: target.project.version,
      orchestrator_worker_id: target.project.orchestrator.id,
      text: buildExecutionOrder(text),
    },
  }
}

async function sendTargetCommand(command: TargetCommand) {
  if (command.kind === 'assignment') {
    await sendAssignmentPrompt(
      command.target.assignment.project_id,
      command.target.assignment.id,
      command.command,
    )
    return
  }
  await sendOrchestratorPrompt(command.target.project.id, command.command)
}

export function AgentGroupChat({
  targets,
}: {
  targets: AgentGroupTarget[]
}) {
  const titleId = useId()
  const messageId = useId()
  const templateId = useId()
  const stableTargets = useMemo(
    () =>
      [...targets].sort((left, right) =>
        agentGroupTargetKey(left).localeCompare(
          agentGroupTargetKey(right),
        ),
      ),
    [targets],
  )
  const groupKey = stableTargets.map(agentGroupTargetKey).join('|')
  const targetsRef = useRef(stableTargets)
  const groupKeyRef = useRef(groupKey)
  const [messages, setMessages] = useState<ThreadMessage[]>([])
  const [snapshotsLoading, setSnapshotsLoading] = useState(false)
  const [promptText, setPromptText] = useState('')
  const [sending, setSending] = useState(false)
  const [open, setOpen] = useState(false)
  const [deliveries, setDeliveries] = useState<
    Record<string, TargetDelivery>
  >({})
  const retainedCommands = useRef<Map<string, TargetCommand>>(new Map())
  const activeText = useRef('')
  const loadedGroup = useRef<string | null>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const dialogRef = useRef<HTMLElement>(null)
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const messagesRef = useRef<HTMLDivElement>(null)
  useModalDialog({
    active: open,
    dialogRef,
    initialFocusRef: textareaRef,
    onClose: () => setOpen(false),
    returnFocus: triggerRef.current,
  })

  const loadSnapshots = useCallback(async () => {
    const requestedGroup = groupKeyRef.current
    setSnapshotsLoading(true)
    const snapshots = await Promise.all(
      targetsRef.current.map(async (target): Promise<ThreadMessage> => {
        const key = agentGroupTargetKey(target)
        try {
          const output =
            target.kind === 'assignment'
              ? await fetchAssignmentTerminalOutput(
                  target.assignment.project_id,
                  target.assignment.id,
                  TERMINAL_OUTPUT_LINES,
                )
              : await fetchOrchestratorTerminalOutput(
                  target.project.id,
                  TERMINAL_OUTPUT_LINES,
                )
          const text = normalizeTerminalOutput(output.text)
          return {
            id: `snapshot:${key}:${output.revision}`,
            kind: 'agent',
            label: target.label,
            meta: `${targetProjectName(target)} · ${target.status}${
              output.truncated
                ? ` · ${TRUNCATED_TERMINAL_OUTPUT_LABEL}`
                : ''
            } · rev ${output.revision}`,
            status: target.status,
            text: text || 'No recent output.',
            truncated: output.truncated,
          }
        } catch (caught) {
          return {
            id: `snapshot-error:${key}`,
            kind: 'error',
            label: target.label,
            meta: targetProjectName(target),
            text: errorMessage(caught, 'Recent output unavailable'),
          }
        }
      }),
    )
    if (groupKeyRef.current === requestedGroup) {
      setMessages((current) => [
        ...current.filter((message) => !message.id.startsWith('snapshot')),
        ...snapshots,
      ])
      setSnapshotsLoading(false)
    }
  }, [])

  useLayoutEffect(() => {
    targetsRef.current = stableTargets
    groupKeyRef.current = groupKey
  }, [groupKey, stableTargets])

  useEffect(() => {
    if (loadedGroup.current === groupKey) return
    loadedGroup.current = groupKey
    setOpen(false)
    setSending(false)
    retainedCommands.current.clear()
    activeText.current = ''
    setDeliveries({})
    setMessages([])
    setPromptText('')
    setSnapshotsLoading(false)
  }, [groupKey])

  useEffect(() => {
    if (!open) return
    void loadSnapshots()
    window.requestAnimationFrame(() => textareaRef.current?.focus())
  }, [loadSnapshots, open])

  useEffect(() => {
    if (!open) return
    const messagesElement = messagesRef.current
    if (messagesElement) {
      messagesElement.scrollTop = messagesElement.scrollHeight
    }
  }, [messages, open])

  const dispatch = async (
    commands: TargetCommand[],
    text: string,
    appendUserMessage: boolean,
  ) => {
    if (commands.length === 0 || sending) return
    const requestedGroup = groupKeyRef.current
    setSending(true)
    if (appendUserMessage) {
      const question = {
        askedAtUnixMs: Date.now(),
        id: crypto.randomUUID(),
        text,
      }
      recordAgentQuestion(
        commands.map((command) =>
          command.target.kind === 'assignment'
            ? agentQuestionKey({
                assignment: command.target.assignment,
                kind: 'assignment',
              })
            : agentQuestionKey({
                kind: 'orchestrator',
                project: command.target.project,
              }),
        ),
        question,
      )
      setMessages((current) => [
        ...current,
        {
          id: `user:${question.id}`,
          kind: 'user',
          label: 'Your question',
          meta: `${commands.length} recipient${commands.length === 1 ? '' : 's'}`,
          question,
          text,
        },
      ])
    }
    setDeliveries((current) => {
      const next = { ...current }
      for (const command of commands) {
        next[agentGroupTargetKey(command.target)] = { state: 'pending' }
      }
      return next
    })

    await Promise.all(
      commands.map(async (command) => {
        const key = agentGroupTargetKey(command.target)
        try {
          await sendTargetCommand(command)
          if (groupKeyRef.current !== requestedGroup) return
          setDeliveries((current) => ({
            ...current,
            [key]: { state: 'delivered' },
          }))
          retainedCommands.current.delete(key)
        } catch (caught) {
          if (groupKeyRef.current !== requestedGroup) return
          setDeliveries((current) => ({
            ...current,
            [key]: {
              state: 'failed',
              error: errorMessage(caught, 'Prompt acknowledgement failed'),
              requiresNewCommand: promptRequiresNewCommand(caught),
            },
          }))
        }
      }),
    )
    if (groupKeyRef.current === requestedGroup) setSending(false)
  }

  const submit = async (event: FormEvent) => {
    event.preventDefault()
    const text = promptText.trim()
    if (!text || sending) return
    activeText.current = text
    const commands = stableTargets.map((target) => {
      const command = commandForTarget(target, text)
      retainedCommands.current.set(agentGroupTargetKey(target), command)
      return command
    })
    await dispatch(commands, text, true)
  }

  const retryFailed = async (fresh: boolean) => {
    const commands = stableTargets.flatMap((target) => {
      const key = agentGroupTargetKey(target)
      const delivery = deliveries[key]
      if (
        delivery?.state !== 'failed' ||
        Boolean(delivery.requiresNewCommand) !== fresh
      ) {
        return []
      }
      if (fresh) {
        const command = commandForTarget(target, activeText.current)
        retainedCommands.current.set(key, command)
        return [command]
      }
      const retained = retainedCommands.current.get(key)
      return retained ? [retained] : []
    })
    await dispatch(commands, activeText.current, false)
  }

  const failed = Object.values(deliveries).filter(
    ({ state }) => state === 'failed',
  )
  const retryableCount = failed.filter(
    ({ requiresNewCommand }) => !requiresNewCommand,
  ).length
  const freshCount = failed.length - retryableCount
  const knownQuestions = messages.flatMap((message) =>
    message.kind === 'user' ? [message.text] : [],
  )

  const updatePrompt = (text: string) => {
    setPromptText(text)
    setDeliveries({})
    retainedCommands.current.clear()
  }

  const handleComposerKeyDown = (
    event: ReactKeyboardEvent<HTMLTextAreaElement>,
  ) => {
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
      event.preventDefault()
      event.currentTarget.form?.requestSubmit()
    }
  }

  return (
    <>
      <div className="inspector__identity group-chat-identity">
        <span className="inspector__icon">
          <Users aria-hidden="true" size={20} />
        </span>
        <div>
          <p className="eyebrow">Selected agents</p>
          <h2>Group chat</h2>
        </div>
      </div>
      <div className="group-chat-roster" aria-label="Selected agents">
        {stableTargets.map((target) => (
          <span
            data-status={target.status}
            key={agentGroupTargetKey(target)}
          >
            <Bot aria-hidden="true" size={13} />
            {target.label}
          </span>
        ))}
      </div>
      <section
        aria-label="Agent controls"
        className="intervention-section agent-controls"
      >
        <p className="eyebrow">Intervention</p>
        <div className="agent-control-launchers">
          <div className="agent-control-launcher">
            <span>
              <MessageSquareText aria-hidden="true" size={17} />
              <span>
                <strong>Group chat</strong>
                <small>{stableTargets.length} selected agents</small>
              </span>
            </span>
            <button
              className="secondary-button"
              onClick={() => setOpen(true)}
              ref={triggerRef}
              type="button"
            >
              <MessageSquareText aria-hidden="true" size={15} />
              Open group chat
            </button>
          </div>
        </div>
      </section>

      {open ? createPortal(
        <div
          className="modal-backdrop chat-backdrop"
          role="presentation"
        >
          <section
            aria-labelledby={titleId}
            aria-modal="true"
            className="chat-workspace"
            ref={dialogRef}
            role="dialog"
          >
            <header className="chat-workspace__header">
              <span className="chat-workspace__icon">
                <Users aria-hidden="true" size={20} />
              </span>
              <div>
                <p className="eyebrow">Agent chat</p>
                <h2 id={titleId}>Group chat</h2>
              </div>
              <span className="status-badge">
                <Bot aria-hidden="true" size={13} />
                {stableTargets.length} agents
              </span>
              <button
                aria-label="Close chat"
                className="icon-button"
                onClick={() => setOpen(false)}
                title="Close chat"
                type="button"
              >
                <X aria-hidden="true" size={18} />
              </button>
            </header>

            <div className="chat-workspace__body">
              <aside className="chat-context group-chat-context">
                <div>
                  <p className="eyebrow">Recipients</p>
                  <strong>{stableTargets.length} selected agents</strong>
                  <small>Broadcast order</small>
                </div>
                <label className="chat-context__field" htmlFor={templateId}>
                  <span className="field-label">Order template</span>
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
                </label>
                <details className="group-chat-context__recipients">
                  <summary>
                    <Users aria-hidden="true" size={13} />
                    <span>{stableTargets.length} recipients</span>
                  </summary>
                  <div
                    aria-label="Selected agents"
                    className="group-chat-roster"
                  >
                    {stableTargets.map((target) => (
                      <span
                        data-status={target.status}
                        key={agentGroupTargetKey(target)}
                      >
                        <Bot aria-hidden="true" size={13} />
                        {target.label}
                      </span>
                    ))}
                  </div>
                </details>
                {Object.keys(deliveries).length > 0 ? (
                  <div className="group-delivery-results" role="status">
                    {stableTargets.map((target) => {
                      const delivery =
                        deliveries[agentGroupTargetKey(target)]
                      if (!delivery) return null
                      return (
                        <div
                          data-state={delivery.state}
                          key={agentGroupTargetKey(target)}
                        >
                          {delivery.state === 'pending' ? (
                            <LoaderCircle
                              aria-hidden="true"
                              className="status-spin"
                              size={14}
                            />
                          ) : delivery.state === 'delivered' ? (
                            <CircleCheck aria-hidden="true" size={14} />
                          ) : (
                            <CircleAlert aria-hidden="true" size={14} />
                          )}
                          <span>
                            <strong>{target.label}</strong>
                            <small>
                              {delivery.state === 'delivered'
                                ? 'Delivered'
                                : delivery.state === 'pending'
                                  ? 'Sending'
                                  : delivery.error}
                            </small>
                          </span>
                        </div>
                      )
                    })}
                    {retryableCount > 0 ? (
                      <button
                        className="secondary-button"
                        disabled={sending}
                        onClick={() => void retryFailed(false)}
                        type="button"
                      >
                        <RefreshCw aria-hidden="true" size={15} />
                        Retry ({retryableCount})
                      </button>
                    ) : null}
                    {freshCount > 0 ? (
                      <button
                        className="secondary-button"
                        disabled={sending}
                        onClick={() => void retryFailed(true)}
                        type="button"
                      >
                        <Send aria-hidden="true" size={15} />
                        Send new ({freshCount})
                      </button>
                    ) : null}
                  </div>
                ) : null}
              </aside>

              <section
                aria-label="Combined recent agent messages"
                className="chat-thread"
              >
                <header className="chat-thread__header">
                  <div>
                    <p className="eyebrow">Transcript</p>
                    <h3>Questions and answers</h3>
                  </div>
                  <button
                    aria-label="Refresh combined thread"
                    className="icon-button"
                    disabled={snapshotsLoading}
                    onClick={() => void loadSnapshots()}
                    title="Refresh combined thread"
                    type="button"
                  >
                    <RefreshCw
                      aria-hidden="true"
                      className={snapshotsLoading ? 'status-spin' : ''}
                      size={16}
                    />
                  </button>
                </header>
                <div
                  aria-live="polite"
                  className="chat-thread__messages"
                  ref={messagesRef}
                >
                  {snapshotsLoading && messages.length === 0 ? (
                    <div className="chat-thread__loading" role="status">
                      <LoaderCircle
                        aria-hidden="true"
                        className="status-spin"
                        size={18}
                      />
                      Loading recent agent activity
                    </div>
                  ) : null}
                  {messages.map((message) => (
                    <article
                      className="chat-message"
                      data-kind={message.kind}
                      data-question-id={message.question?.id}
                      key={message.id}
                    >
                      {message.kind === 'agent' ? (
                        <>
                          <header>
                            <strong>{message.label}</strong>
                            <small>{message.meta}</small>
                          </header>
                          <CollapsibleAgentOutput
                            agentLabel={message.label}
                            knownQuestions={knownQuestions}
                            status={message.status}
                            text={message.text}
                            truncated={message.truncated}
                          />
                        </>
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
              onSubmit={(event) => void submit(event)}
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
                  placeholder="Give every selected agent the same order..."
                  ref={textareaRef}
                  rows={3}
                  value={promptText}
                />
                <button
                  aria-label={`Send to ${stableTargets.length}`}
                  className="command-button"
                  disabled={sending || !promptText.trim()}
                  title={`Send to ${stableTargets.length} agents`}
                  type="submit"
                >
                  {sending ? (
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
            </form>
          </section>
        </div>,
        document.body,
      ) : null}
    </>
  )
}

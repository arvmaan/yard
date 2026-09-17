import {
  CheckCircle2,
  ChevronRight,
  Copy,
  FileText,
  MessageCircleQuestion,
  Wrench,
} from 'lucide-react'
import {
  lazy,
  Suspense,
  useEffect,
  useState,
  type ReactNode,
} from 'react'
import {
  parseTerminalTranscript,
  type TerminalTranscriptTurn,
} from './terminalOutput'
import type { ObservedStatus } from './types'

const AgentMarkdown = lazy(() => import('./AgentMarkdown'))

function comparableQuestion(text: string) {
  return text.trim().replace(/\s+/g, ' ')
}

function questionIsKnown(
  question: string,
  knownQuestions: readonly string[],
) {
  const candidate = comparableQuestion(question)
  return knownQuestions.some((knownQuestion) => {
    const known = comparableQuestion(knownQuestion)
    return Boolean(known) &&
      (candidate === known || candidate.startsWith(`${known} `))
  })
}

function compactPreview(text: string) {
  return text.trim().replace(/\s+/g, ' ')
}

function lineLabel(text: string) {
  const lineCount = text.split('\n').length
  return `${lineCount} line${lineCount === 1 ? '' : 's'}`
}

function MarkdownContent({ text }: { text: string }) {
  return (
    <Suspense
      fallback={
        <div className="agent-output__markdown-loading">
          Rendering response
        </div>
      }
    >
      <AgentMarkdown text={text} />
    </Suspense>
  )
}

function CollapsibleEntry({
  children,
  kind,
  summary,
}: {
  children: ReactNode
  kind: 'answer' | 'question'
  summary: ReactNode
}) {
  const [open, setOpen] = useState(false)
  return (
    <details
      className="agent-output__entry"
      data-kind={kind}
      onToggle={(event) => setOpen(event.currentTarget.open)}
      open={open}
    >
      {summary}
      {open ? children : null}
    </details>
  )
}

function FullTranscript({ text }: { text: string }) {
  return (
    <details className="agent-output__transcript">
      <summary>
        <FileText aria-hidden="true" size={13} />
        <span>Terminal detail</span>
        <small>{lineLabel(text)}</small>
      </summary>
      <pre>{text}</pre>
    </details>
  )
}

function PreformattedOutput({ text }: { text: string }) {
  return <pre className="agent-output__preformatted">{text}</pre>
}

export function CollapsibleQuestion({
  meta,
  text,
}: {
  meta?: string
  text: string
}) {
  return (
    <CollapsibleEntry
      kind="question"
      summary={
        <summary>
          <ChevronRight aria-hidden="true" size={13} />
          <MessageCircleQuestion aria-hidden="true" size={13} />
          <strong>Question</strong>
          <span>{compactPreview(text)}</span>
          {meta ? <small>{meta}</small> : null}
        </summary>
      }
    >
      <div className="agent-output__question-text">
        <MarkdownContent text={text} />
      </div>
    </CollapsibleEntry>
  )
}

function CollapsibleAnswer({
  turn,
}: {
  turn: TerminalTranscriptTurn
}) {
  const totalOutput = [turn.work, turn.answer].filter(Boolean).join('\n\n')
  const preview = compactPreview(turn.answer || turn.work)
  if (!totalOutput) return null

  const label = turn.answer ? 'Answer' : 'Activity'
  return (
    <CollapsibleEntry
      kind="answer"
      summary={
        <summary>
          <ChevronRight aria-hidden="true" size={13} />
          {turn.answer ? (
            <CheckCircle2 aria-hidden="true" size={13} />
          ) : (
            <Wrench aria-hidden="true" size={13} />
          )}
          <strong>{label}</strong>
          <span>{preview}</span>
          <small>{lineLabel(totalOutput)}</small>
        </summary>
      }
    >
      <div className="agent-output__entry-body">
        <section className="agent-output__final-answer">
          <span className="agent-output__section-label">{label}</span>
          {turn.answer ? (
            <MarkdownContent text={turn.answer} />
          ) : (
            <PreformattedOutput text={turn.work} />
          )}
        </section>
        {turn.answer && turn.work ? (
          <FullTranscript text={turn.work} />
        ) : null}
      </div>
    </CollapsibleEntry>
  )
}

function PrimaryAnswer({
  agentLabel,
  turn,
}: {
  agentLabel?: string
  turn: TerminalTranscriptTurn
}) {
  const text = turn.answer || turn.work
  const label = turn.answer ? 'Answer' : 'Activity'
  const copySubject = `${agentLabel ? `${agentLabel} ` : ''}${
    turn.answer ? 'answer' : 'terminal output'
  }`
  const [copyState, setCopyState] = useState<
    'idle' | 'copied' | 'failed'
  >('idle')
  useEffect(() => setCopyState('idle'), [text])

  if (!text) return null

  const copy = async () => {
    try {
      if (!navigator.clipboard?.writeText) {
        throw new Error('Clipboard unavailable')
      }
      await navigator.clipboard.writeText(text)
      setCopyState('copied')
    } catch {
      setCopyState('failed')
    }
  }

  return (
    <section
      aria-label={turn.answer ? 'Latest answer' : 'Latest activity'}
      className="agent-output__primary"
      data-kind={turn.answer ? 'answer' : 'activity'}
    >
      <header>
        <span>
          {turn.answer ? (
            <CheckCircle2 aria-hidden="true" size={14} />
          ) : (
            <Wrench aria-hidden="true" size={14} />
          )}
          <strong>{label}</strong>
          <small>{lineLabel(text)}</small>
        </span>
        <button
          aria-label={`Copy ${copySubject}`}
          className="agent-output__copy"
          data-state={copyState}
          onClick={() => void copy()}
          type="button"
        >
          <Copy aria-hidden="true" size={13} />
          <span>
            {copyState === 'copied'
              ? 'Copied'
              : copyState === 'failed'
                ? 'Copy unavailable'
                : 'Copy'}
          </span>
        </button>
      </header>
      <span
        aria-live="polite"
        className="visually-hidden"
        role="status"
      >
        {copyState === 'copied'
          ? `${copySubject[0].toUpperCase()}${copySubject.slice(1)} copied to clipboard.`
          : copyState === 'failed'
            ? 'Copy unavailable. Select and copy manually.'
            : ''}
      </span>
      <div className="agent-output__primary-body">
        {copyState === 'failed' ? (
          <label className="agent-output__copy-fallback">
            <span>Clipboard unavailable. Select and copy manually.</span>
            <textarea
              aria-label={`Manual copy ${copySubject}`}
              onFocus={(event) => event.currentTarget.select()}
              readOnly
              rows={Math.min(5, Math.max(2, text.split('\n').length))}
              value={text}
            />
          </label>
        ) : null}
        {turn.answer ? (
          <MarkdownContent text={turn.answer} />
        ) : (
          <PreformattedOutput text={turn.work} />
        )}
        {turn.answer && turn.work ? (
          <FullTranscript text={turn.work} />
        ) : null}
      </div>
    </section>
  )
}

function AgentTurn({
  agentLabel,
  knownQuestions,
  primary,
  turn,
}: {
  agentLabel?: string
  knownQuestions: readonly string[]
  primary: boolean
  turn: TerminalTranscriptTurn
}) {
  const showQuestion =
    Boolean(turn.question) &&
    !questionIsKnown(turn.question, knownQuestions)
  return (
    <>
      {showQuestion ? (
        <CollapsibleQuestion text={turn.question} />
      ) : null}
      {primary ? (
        <PrimaryAnswer agentLabel={agentLabel} turn={turn} />
      ) : (
        <CollapsibleAnswer turn={turn} />
      )}
    </>
  )
}

function RawOutput({
  agentLabel,
  text,
}: {
  agentLabel?: string
  text: string
}) {
  return (
    <PrimaryAnswer
      agentLabel={agentLabel}
      turn={{
        answer: '',
        question: '',
        work: text,
      }}
    />
  )
}

export function CollapsibleAgentOutput({
  agentLabel,
  knownQuestions = [],
  status = 'unknown',
  text,
  truncated = false,
}: {
  agentLabel?: string
  knownQuestions?: readonly string[]
  status?: ObservedStatus
  text: string
  truncated?: boolean
}) {
  const transcript = parseTerminalTranscript(text, status, truncated)
  const hasVisibleContent = transcript.turns.some(
    (turn) =>
      Boolean(turn.question) || Boolean(turn.work) || Boolean(turn.answer),
  )
  if (!transcript.structured || !hasVisibleContent) {
    return <RawOutput agentLabel={agentLabel} text={text} />
  }
  const primaryIndex = transcript.turns.reduce(
    (latest, turn, index) => turn.answer ? index : latest,
    -1,
  )

  return (
    <div className="agent-output">
      {transcript.turns.map((turn, index) => (
        <AgentTurn
          agentLabel={agentLabel}
          key={`${index}:${turn.question}`}
          knownQuestions={knownQuestions}
          primary={
            index === (
              primaryIndex >= 0
                ? primaryIndex
                : transcript.turns.length - 1
            )
          }
          turn={turn}
        />
      ))}
    </div>
  )
}

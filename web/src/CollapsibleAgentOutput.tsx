import {
  CheckCircle2,
  ChevronRight,
  FileText,
  MessageCircleQuestion,
  Wrench,
} from 'lucide-react'
import {
  lazy,
  Suspense,
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
        <span>Full terminal transcript</span>
        <small>{lineLabel(text)}</small>
      </summary>
      <pre>{text}</pre>
    </details>
  )
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
          <strong>Answer</strong>
          <span>{preview}</span>
          <small>{lineLabel(totalOutput)}</small>
        </summary>
      }
    >
      <div className="agent-output__entry-body">
        {turn.answer ? (
          <section className="agent-output__final-answer">
            <span className="agent-output__section-label">Response</span>
            <MarkdownContent text={turn.answer} />
          </section>
        ) : null}
        {turn.work ? (
          <FullTranscript text={totalOutput} />
        ) : null}
      </div>
    </CollapsibleEntry>
  )
}

function AgentTurn({
  knownQuestions,
  turn,
}: {
  knownQuestions: readonly string[]
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
      <CollapsibleAnswer turn={turn} />
    </>
  )
}

function RawOutput({ text }: { text: string }) {
  return (
    <CollapsibleEntry
      kind="answer"
      summary={
        <summary>
          <ChevronRight aria-hidden="true" size={13} />
          <CheckCircle2 aria-hidden="true" size={13} />
          <strong>Answer</strong>
          <span>{compactPreview(text)}</span>
          <small>{lineLabel(text)}</small>
        </summary>
      }
    >
      <div className="agent-output__entry-body">
        <section className="agent-output__final-answer">
          <span className="agent-output__section-label">Response</span>
          <MarkdownContent text={text} />
        </section>
        <FullTranscript text={text} />
      </div>
    </CollapsibleEntry>
  )
}

export function CollapsibleAgentOutput({
  knownQuestions = [],
  status = 'unknown',
  text,
  truncated = false,
}: {
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
    return <RawOutput text={text} />
  }

  return (
    <div className="agent-output">
      {transcript.turns.map((turn, index) => (
        <AgentTurn
          key={`${index}:${turn.question}`}
          knownQuestions={knownQuestions}
          turn={turn}
        />
      ))}
    </div>
  )
}

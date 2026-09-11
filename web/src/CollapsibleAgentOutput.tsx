import {
  CheckCircle2,
  ChevronRight,
  MessageCircleQuestion,
  Wrench,
} from 'lucide-react'
import {
  parseTerminalTranscript,
  type TerminalTranscriptTurn,
} from './terminalOutput'
import type { ObservedStatus } from './types'

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

export function CollapsibleQuestion({
  meta,
  text,
}: {
  meta?: string
  text: string
}) {
  return (
    <details className="agent-output__entry" data-kind="question">
      <summary>
        <ChevronRight aria-hidden="true" size={13} />
        <MessageCircleQuestion aria-hidden="true" size={13} />
        <strong>Question</strong>
        <span>{compactPreview(text)}</span>
        {meta ? <small>{meta}</small> : null}
      </summary>
      <p className="agent-output__question-text">{text}</p>
    </details>
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
    <details className="agent-output__entry" data-kind="answer">
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
      <div className="agent-output__entry-body">
        {turn.answer ? (
          <section className="agent-output__final-answer">
            <strong>Answer</strong>
            <p>{turn.answer}</p>
          </section>
        ) : null}
        {turn.work ? (
          <section className="agent-output__total-output">
            <strong>Total output</strong>
            <pre>{totalOutput}</pre>
          </section>
        ) : null}
      </div>
    </details>
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
    <details className="agent-output__entry" data-kind="answer">
      <summary>
        <ChevronRight aria-hidden="true" size={13} />
        <CheckCircle2 aria-hidden="true" size={13} />
        <strong>Answer</strong>
        <span>{compactPreview(text)}</span>
        <small>{lineLabel(text)}</small>
      </summary>
      <pre className="agent-output__raw">{text}</pre>
    </details>
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

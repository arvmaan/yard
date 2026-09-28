import { useEffect, useState } from 'react'
import { History, LoaderCircle, ScrollText } from 'lucide-react'
import { fetchAssignmentTranscript } from './api'
import type {
  Assignment,
  TranscriptUnavailableReason,
  WorkerTranscript,
} from './types'

const PENDING_REFRESH_INTERVAL_MS = 5_000

const UNAVAILABLE_REASONS: Record<TranscriptUnavailableReason, string> = {
  not_captured: 'Yard did not capture this terminal when the work ended.',
  runtime_closed: 'The Herdr tab closed before Yard could read it.',
  runtime_reused:
    'The terminal was reused by another agent, so Yard did not read it.',
  worker_reallocated:
    'The worker took new work before its terminal could be read.',
}

/**
 * The read-only terminal transcript Yard retained when this assignment
 * ended. Live terminal access is revoked at completion, so this is the
 * durable record; the provider session is named as the fallback when no
 * text could be captured.
 */
export function AssignmentTranscript({
  assignment,
}: {
  assignment: Assignment
}) {
  const [transcript, setTranscript] = useState<WorkerTranscript | null>(null)
  const [error, setError] = useState<string | null>(null)
  const pending = transcript?.status === 'pending'

  useEffect(() => {
    const controller = new AbortController()
    fetchAssignmentTranscript(
      assignment.project_id,
      assignment.id,
      controller.signal,
    )
      .then((loaded) => {
        setTranscript(loaded)
        setError(null)
      })
      .catch((caught: unknown) => {
        if (controller.signal.aborted) return
        setError(
          caught instanceof Error ? caught.message : 'Transcript unavailable',
        )
      })
    return () => controller.abort()
  }, [assignment.id, assignment.project_id, assignment.version])

  useEffect(() => {
    if (!pending) return
    const controller = new AbortController()
    const interval = window.setInterval(() => {
      if (document.visibilityState !== 'visible') return
      void fetchAssignmentTranscript(
        assignment.project_id,
        assignment.id,
        controller.signal,
      )
        .then(setTranscript)
        .catch(() => undefined)
    }, PENDING_REFRESH_INTERVAL_MS)
    return () => {
      window.clearInterval(interval)
      controller.abort()
    }
  }, [assignment.id, assignment.project_id, pending])

  const provider = transcript?.provider_session

  return (
    <section
      aria-label="Transcript"
      className="intervention-section retained-transcript"
    >
      <p className="eyebrow">Transcript</p>
      {!transcript && !error ? (
        <p className="retained-transcript__status" role="status">
          <LoaderCircle aria-hidden="true" className="status-spin" size={15} />
          Loading transcript…
        </p>
      ) : transcript?.status === 'captured' && transcript.text !== null ? (
        <>
          <p className="retained-transcript__meta">
            <ScrollText aria-hidden="true" size={15} />
            <span>
              Read-only · {transcript.line_count} lines
              {transcript.truncated ? ' (most recent kept)' : ''}
              {transcript.captured_at_unix_ms
                ? ` · captured ${new Date(transcript.captured_at_unix_ms).toISOString()}`
                : ''}
            </span>
          </p>
          <pre
            aria-label="Retained terminal transcript"
            className="retained-transcript__text"
            tabIndex={0}
          >
            {transcript.text}
          </pre>
        </>
      ) : transcript?.status === 'pending' ? (
        <p className="retained-transcript__status" role="status">
          <History aria-hidden="true" size={15} />
          <span>
            Saving the transcript
            {transcript.attempts > 0
              ? ' — Herdr was unreachable; Yard keeps retrying.'
              : '…'}
          </span>
        </p>
      ) : (
        <div className="retained-transcript__status" role="status">
          <History aria-hidden="true" size={15} />
          <span>
            <strong>Transcript unavailable</strong>
            <small>
              {error ??
                (transcript?.unavailable_reason
                  ? UNAVAILABLE_REASONS[transcript.unavailable_reason]
                  : 'No transcript was retained.')}
            </small>
            {provider ? (
              <small>
                Provider session: {provider.provider} {provider.kind}{' '}
                <code>{provider.value}</code>
              </small>
            ) : null}
          </span>
        </div>
      )}
    </section>
  )
}

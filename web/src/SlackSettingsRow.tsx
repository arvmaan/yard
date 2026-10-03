import { useCallback, useEffect, useRef, useState } from 'react'
import { LoaderCircle, Send } from 'lucide-react'

import { fetchSlackIntegration, sendSlackTestMessage, YardApiError } from './api'
import { slackStatusSummary } from './slackStatus'
import type { SlackIntegrationStatus } from './types'

const SLACK_STATUS_REPOLL_MS = 3_000

/** Settings row: Slack DM notification status and a test button. */
export function SlackSettingsRow() {
  const [status, setStatus] = useState<SlackIntegrationStatus | null>(null)
  const [unavailable, setUnavailable] = useState(false)
  const [sending, setSending] = useState(false)
  const [feedback, setFeedback] = useState<{
    kind: 'success' | 'error'
    message: string
  } | null>(null)
  const controller = useRef<AbortController | null>(null)

  useEffect(() => {
    const abort = new AbortController()
    controller.current = abort
    let timer: number | null = null
    // Re-poll while the first connection is still in progress, so the row
    // moves from "Connecting…" to the result without reopening Settings.
    const load = () => {
      fetchSlackIntegration(abort.signal)
        .then((next) => {
          setStatus(next)
          setUnavailable(false)
          if (next.status === 'connecting') {
            timer = window.setTimeout(load, SLACK_STATUS_REPOLL_MS)
          }
        })
        .catch(() => {
          if (!abort.signal.aborted) setUnavailable(true)
        })
    }
    load()
    return () => {
      abort.abort()
      if (timer !== null) window.clearTimeout(timer)
    }
  }, [])

  const sendTest = useCallback(async () => {
    setSending(true)
    setFeedback(null)
    try {
      const next = await sendSlackTestMessage(controller.current?.signal)
      setStatus(next)
      setUnavailable(false)
      setFeedback({ kind: 'success', message: 'Test message sent. Check your Slack DMs.' })
    } catch (caught) {
      if (controller.current?.signal.aborted) return
      setFeedback({
        kind: 'error',
        message:
          caught instanceof YardApiError
            ? caught.message
            : 'The test message could not be sent',
      })
      try {
        setStatus(await fetchSlackIntegration(controller.current?.signal))
      } catch {
        // Keep the previous status; the feedback line explains the failure.
      }
    } finally {
      setSending(false)
    }
  }, [])

  const summary = slackStatusSummary(status, unavailable)
  return (
    <div className="settings-row" data-slack-status={status?.status ?? 'unknown'}>
      <div>
        <strong>Slack DMs</strong>
        <small data-tone={summary.tone}>{summary.text}</small>
        {feedback ? (
          <small
            className={`settings-feedback settings-feedback--${feedback.kind}`}
            role={feedback.kind === 'error' ? 'alert' : 'status'}
          >
            {feedback.message}
          </small>
        ) : null}
      </div>
      <button
        aria-label="Send Slack test message"
        className="secondary-button settings-action"
        disabled={!summary.canTest || sending}
        onClick={() => void sendTest()}
        type="button"
      >
        {sending ? (
          <LoaderCircle aria-hidden="true" className="status-spin" size={14} />
        ) : (
          <Send aria-hidden="true" size={14} />
        )}
        Send test message
      </button>
    </div>
  )
}

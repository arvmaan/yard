import type { SlackIntegrationStatus } from './types'

export interface SlackStatusSummary {
  /** One line for the Settings row. */
  text: string
  /** Whether "Send test message" can do anything useful. */
  canTest: boolean
  tone: 'quiet' | 'ok' | 'attention'
}

const MAX_ERROR_CHARS = 160

function shorten(text: string): string {
  const trimmed = text.trim()
  return trimmed.length > MAX_ERROR_CHARS
    ? `${trimmed.slice(0, MAX_ERROR_CHARS - 1)}…`
    : trimmed
}

function sentAt(unixMs: number): string {
  return new Date(unixMs).toLocaleString(undefined, {
    dateStyle: 'medium',
    timeStyle: 'short',
  })
}

export function slackStatusSummary(
  status: SlackIntegrationStatus | null,
  unavailable = false,
): SlackStatusSummary {
  if (unavailable) {
    return { text: 'Status unavailable', canTest: false, tone: 'quiet' }
  }
  if (status === null) {
    return { text: 'Checking…', canTest: false, tone: 'quiet' }
  }
  const lastSent =
    status.last_sent_at === null
      ? ''
      : ` · last message ${sentAt(status.last_sent_at)}`
  switch (status.status) {
    case 'off':
      return {
        text: 'Off. Set YARD_SLACK_NOTIFICATIONS=on and restart Yard to get DMs',
        canTest: false,
        tone: 'quiet',
      }
    case 'misconfigured':
      // A runtime problem (the secret's contents, the enterprise, a Slack
      // setting) is fixed without a restart; the test button retries now.
      return status.restart_required
        ? {
            text: `Misconfigured: ${shorten(status.last_error ?? 'check the YARD_SLACK_* settings')}. Fix the setting and restart Yard`,
            canTest: false,
            tone: 'attention',
          }
        : {
            text: `Misconfigured: ${shorten(status.last_error ?? 'check the bot token secret')}. After fixing it, send a test message to retry; no restart needed`,
            canTest: true,
            tone: 'attention',
          }
    case 'connecting':
      return { text: 'Connecting to Slack…', canTest: false, tone: 'quiet' }
    case 'connected':
      return {
        text: `Connected to ${status.team?.name || status.team?.id || 'Slack'}${lastSent}`,
        canTest: true,
        tone: 'ok',
      }
    case 'error':
      return {
        text: `Not delivering: ${shorten(status.last_error ?? 'unknown error')}${lastSent}`,
        canTest: true,
        tone: 'attention',
      }
  }
}

import { useEffect } from 'react'

import { reportSlackPresence } from './api'
import type { SlackPresenceTarget } from './types'

/**
 * How often an open chat repeats its presence report. The server keeps an
 * agent "in view" for 60 s after each report, so three missed beats (a
 * closed chat, a hidden tab, a closed window) end the view.
 */
export const SLACK_PRESENCE_HEARTBEAT_MS = 20_000

function presenceKey(targets: SlackPresenceTarget[]): string {
  return JSON.stringify(targets)
}

/**
 * While `active` and the tab is visible, report `targets` to Yard every
 * {@link SLACK_PRESENCE_HEARTBEAT_MS} so Slack does not DM about an agent
 * the owner is already looking at. Failures are ignored: presence only
 * suppresses notifications, so a missed report at worst sends one.
 */
export function useSlackPresence(
  targets: SlackPresenceTarget[],
  active: boolean,
) {
  const key = presenceKey(targets)
  useEffect(() => {
    const current = JSON.parse(key) as SlackPresenceTarget[]
    if (!active || current.length === 0) return
    let controller: AbortController | null = null
    const beat = () => {
      if (document.visibilityState !== 'visible') return
      controller?.abort()
      controller = new AbortController()
      reportSlackPresence(current, controller.signal).catch(() => {
        // Best effort; see the hook comment.
      })
    }
    beat()
    const timer = window.setInterval(beat, SLACK_PRESENCE_HEARTBEAT_MS)
    document.addEventListener('visibilitychange', beat)
    return () => {
      window.clearInterval(timer)
      document.removeEventListener('visibilitychange', beat)
      controller?.abort()
    }
  }, [active, key])
}

import { describe, expect, it } from 'vitest'

import { slackStatusSummary } from './slackStatus'
import type { SlackIntegrationStatus } from './types'

const base: SlackIntegrationStatus = {
  enabled: true,
  status: 'connected',
  team: { id: 'T01', name: 'Acme Sandbox', enterprise_id: 'E01' },
  last_error: null,
  last_sent_at: null,
  restart_required: false,
}

describe('slackStatusSummary', () => {
  it('describes loading and unavailable states without a test button', () => {
    expect(slackStatusSummary(null)).toEqual({
      text: 'Checking…',
      canTest: false,
      tone: 'quiet',
    })
    expect(slackStatusSummary(base, true).text).toBe('Status unavailable')
    expect(slackStatusSummary(base, true).canTest).toBe(false)
  })

  it('only offers a test message once notifications are configured', () => {
    expect(
      slackStatusSummary({ ...base, enabled: false, status: 'off', team: null })
        .canTest,
    ).toBe(false)
    const misconfigured = slackStatusSummary({
      ...base,
      status: 'misconfigured',
      team: null,
      last_error: 'YARD_SLACK_OWNER_USER_ID is required',
      restart_required: true,
    })
    expect(misconfigured.canTest).toBe(false)
    expect(misconfigured.text).toBe(
      'Misconfigured: YARD_SLACK_OWNER_USER_ID is required. Fix the setting and restart Yard',
    )
    expect(slackStatusSummary(base)).toEqual({
      text: 'Connected to Acme Sandbox',
      canTest: true,
      tone: 'ok',
    })
    const failing = slackStatusSummary({
      ...base,
      status: 'error',
      last_error: 'x'.repeat(400),
    })
    expect(failing.canTest).toBe(true)
    expect(failing.tone).toBe('attention')
    expect(failing.text.length).toBeLessThan(200)
  })

  it('lets the owner retry a runtime misconfiguration without a restart', () => {
    const wrongToken = slackStatusSummary({
      ...base,
      status: 'misconfigured',
      team: null,
      last_error: 'The secret does not hold a bot token (xoxb-…)',
      restart_required: false,
    })
    expect(wrongToken.canTest).toBe(true)
    expect(wrongToken.tone).toBe('attention')
    expect(wrongToken.text).toContain('no restart needed')
  })

  it('shows the first connection attempt neutrally', () => {
    expect(
      slackStatusSummary({ ...base, status: 'connecting', team: null }),
    ).toEqual({
      text: 'Connecting to Slack…',
      canTest: false,
      tone: 'quiet',
    })
  })

  it('mentions when the last message was sent', () => {
    const summary = slackStatusSummary({
      ...base,
      last_sent_at: Date.UTC(2026, 8, 28, 17, 5),
    })
    expect(summary.text).toMatch(/^Connected to Acme Sandbox · last message /)
  })
})

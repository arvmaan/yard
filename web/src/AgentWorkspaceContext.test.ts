import { describe, expect, it } from 'vitest'
import {
  DEFAULT_TERMINAL_PRESENTATION,
  isTerminalWorkspaceMode,
} from './AgentWorkspaceContext'

describe('agent workspace terminal presentation', () => {
  it('defaults single-target terminal work to the full terminal presentation', () => {
    expect(DEFAULT_TERMINAL_PRESENTATION).toBe('terminal')
  })

  it('keeps presentation changes inside terminal workspace mode', () => {
    expect(isTerminalWorkspaceMode('terminal')).toBe(true)
    expect(isTerminalWorkspaceMode('chat')).toBe(false)
    expect(isTerminalWorkspaceMode('map')).toBe(false)
  })
})

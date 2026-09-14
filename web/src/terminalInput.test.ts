import { describe, expect, it } from 'vitest'
import {
  chunkTerminalInput,
  TERMINAL_INPUT_CHUNK_BYTES,
} from './terminalInput'

const encoder = new TextEncoder()

describe('chunkTerminalInput', () => {
  it('preserves large terminal pastes in bounded chunks', () => {
    const input = 'terminal paste\n'.repeat(2_000)
    const chunks = chunkTerminalInput(input)

    expect(chunks.length).toBeGreaterThan(1)
    expect(chunks.join('')).toBe(input)
    expect(
      chunks.every(
        (chunk) =>
          encoder.encode(chunk).byteLength <= TERMINAL_INPUT_CHUNK_BYTES,
      ),
    ).toBe(true)
  })

  it('keeps multibyte characters intact at chunk boundaries', () => {
    const input = `${'a'.repeat(TERMINAL_INPUT_CHUNK_BYTES - 1)}🚀rest`
    const chunks = chunkTerminalInput(input)

    expect(chunks).toEqual([
      'a'.repeat(TERMINAL_INPUT_CHUNK_BYTES - 1),
      '🚀rest',
    ])
    expect(chunks.join('')).toBe(input)
  })

  it('does not create input for an empty event', () => {
    expect(chunkTerminalInput('')).toEqual([])
  })
})

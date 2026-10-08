import { afterEach, describe, expect, it, vi } from 'vitest'
import { copyMachineCommand } from './machineClipboard'

const originalNavigator = globalThis.navigator

function clipboard(value: unknown) {
  Object.defineProperty(globalThis, 'navigator', {
    configurable: true,
    value: { clipboard: value },
  })
}

afterEach(() => {
  Object.defineProperty(globalThis, 'navigator', {
    configurable: true,
    value: originalNavigator,
  })
})

describe('machine command clipboard', () => {
  it('copies through the Clipboard API with its receiver', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    const api = { writeText }
    clipboard(api)

    await expect(copyMachineCommand("herdr machine add -- 'host'")).resolves.toBeUndefined()
    expect(writeText).toHaveBeenCalledWith("herdr machine add -- 'host'")
    expect(writeText.mock.instances[0]).toBe(api)
  })

  it('rejects normally when Clipboard API is absent', async () => {
    clipboard(undefined)
    await expect(copyMachineCommand('selectable command')).rejects.toThrow(
      'Clipboard is unavailable.',
    )
  })

  it('propagates a normal clipboard rejection for UI sanitization', async () => {
    clipboard({ writeText: vi.fn().mockRejectedValue(new Error('browser detail')) })
    await expect(copyMachineCommand('selectable command')).rejects.toThrow(
      'browser detail',
    )
  })
})

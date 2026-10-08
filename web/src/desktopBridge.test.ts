import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  hasDesktopMachineHandoff,
  launchMachineAdd,
  launchMachineReconnect,
} from './desktopBridge'

const desktopGlobal = globalThis as typeof globalThis & {
  __TAURI_INTERNALS__?: { invoke: ReturnType<typeof vi.fn> }
}

afterEach(() => {
  delete desktopGlobal.__TAURI_INTERNALS__
})

describe('desktop machine handoff bridge', () => {
  it('feature-detects Tauri and sends typed add fields, never a command string', async () => {
    const invoke = vi.fn().mockResolvedValue({ launched: true })
    desktopGlobal.__TAURI_INTERNALS__ = { invoke }

    expect(hasDesktopMachineHandoff()).toBe(true)
    await launchMachineAdd({
      ssh_target: 'user@host',
      label: 'Build box',
      session: 'release',
    })

    expect(invoke).toHaveBeenCalledWith('launch_machine_add', {
      request: {
        ssh_target: 'user@host',
        label: 'Build box',
        session: 'release',
      },
    })
    expect(JSON.stringify(invoke.mock.calls)).not.toContain('herdr machine')
  })

  it('reconnects using the stable machine ID field', async () => {
    const invoke = vi.fn().mockResolvedValue({ launched: true })
    desktopGlobal.__TAURI_INTERNALS__ = { invoke }
    const machine_id = '0123456789abcdef0123456789abcdef'

    await launchMachineReconnect({ machine_id })

    expect(invoke).toHaveBeenCalledWith('launch_machine_reconnect', {
      request: { machine_id },
    })
  })

  it('reports browser mode and rejects malformed receipts', async () => {
    expect(hasDesktopMachineHandoff()).toBe(false)
    await expect(
      launchMachineReconnect({
        machine_id: '0123456789abcdef0123456789abcdef',
      }),
    ).rejects.toThrow('unavailable')

    desktopGlobal.__TAURI_INTERNALS__ = {
      invoke: vi.fn().mockResolvedValue({ launched: false }),
    }
    await expect(
      launchMachineReconnect({
        machine_id: '0123456789abcdef0123456789abcdef',
      }),
    ).rejects.toThrow('did not launch')
  })
})

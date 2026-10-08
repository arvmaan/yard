export type AddMachineDesktopRequest = {
  ssh_target: string
  label?: string
  session?: string
}

export type ReconnectMachineDesktopRequest = {
  machine_id: string
}

type Invoke = <T>(command: string, payload?: Record<string, unknown>) => Promise<T>

type TauriInternals = {
  invoke?: Invoke
}

declare global {
  interface Window {
    __TAURI_INTERNALS__?: TauriInternals
  }
}

type DesktopGlobal = typeof globalThis & {
  __TAURI_INTERNALS__?: TauriInternals
}

const invoke = (): Invoke | null => {
  const internals = (globalThis as DesktopGlobal).__TAURI_INTERNALS__
  const candidate = internals?.invoke
  return typeof candidate === 'function' ? candidate.bind(internals) : null
}

export const hasDesktopMachineHandoff = () => invoke() !== null

async function launch(
  command: 'launch_machine_add' | 'launch_machine_reconnect',
  request: AddMachineDesktopRequest | ReconnectMachineDesktopRequest,
) {
  const desktopInvoke = invoke()
  if (!desktopInvoke) {
    throw Error('Desktop Terminal handoff is unavailable.')
  }
  const receipt = await desktopInvoke<{ launched?: unknown }>(command, { request })
  if (receipt?.launched !== true) {
    throw Error('Desktop Terminal handoff did not launch.')
  }
}

export const launchMachineAdd = (request: AddMachineDesktopRequest) =>
  launch('launch_machine_add', request)

export const launchMachineReconnect = (request: ReconnectMachineDesktopRequest) =>
  launch('launch_machine_reconnect', request)

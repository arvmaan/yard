import type {
  EndpointRuntimeInventory,
  RuntimeEndpoint,
  RuntimeEndpointConnectionState,
  RuntimeEndpointRef,
  RuntimeEndpoints,
  RuntimeInventory,
} from './types'

const ID = /^[0-9a-f]{32}$/
const STATES = new Set<RuntimeEndpointConnectionState>([
  'reachable',
  'disabled',
  'authentication_required',
  'unreachable',
  'incompatible',
  'unknown',
])

const record = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value)

const control = (character: string) => {
  const code = character.charCodeAt(0)
  return code < 32 || code === 127
}

const text = (value: unknown) =>
  typeof value === 'string' &&
  value.length > 0 &&
  new TextEncoder().encode(value).length <= 128 &&
  ![...value].some(control)

export const isStableMachineId = (value: string) => ID.test(value)

export const endpointKey = (endpoint: RuntimeEndpointRef) =>
  endpoint.kind === 'local' ? 'local' : `machine:${endpoint.machine_id}`

function ref(value: unknown): RuntimeEndpointRef {
  if (!record(value)) {
    throw Error('Invalid endpoint identity')
  }
  if (value.kind === 'local') {
    return { kind: 'local' }
  }
  if (
    value.kind === 'machine' &&
    typeof value.machine_id === 'string' &&
    ID.test(value.machine_id)
  ) {
    return { kind: 'machine', machine_id: value.machine_id }
  }
  throw Error('Invalid endpoint identity')
}

function endpoint(value: unknown): RuntimeEndpoint {
  if (
    !record(value) ||
    !text(value.label) ||
    typeof value.enabled !== 'boolean' ||
    typeof value.connection_state !== 'string' ||
    !STATES.has(value.connection_state as RuntimeEndpointConnectionState) ||
    !record(value.capabilities) ||
    typeof value.capabilities.inventory_read !== 'boolean' ||
    typeof value.capabilities.mutations !== 'boolean' ||
    typeof value.capabilities.terminal_streaming !== 'boolean' ||
    !Array.isArray(value.sessions) ||
    value.sessions.length > 64
  ) {
    throw Error('Invalid endpoint')
  }

  const sessions = value.sessions.map((sessionValue) => {
    if (
      !record(sessionValue) ||
      !text(sessionValue.name) ||
      typeof sessionValue.is_default !== 'boolean' ||
      !(
        typeof sessionValue.observed_running === 'boolean' ||
        sessionValue.observed_running === null
      )
    ) {
      throw Error('Invalid session')
    }
    return {
      name: sessionValue.name as string,
      is_default: sessionValue.is_default,
      observed_running: sessionValue.observed_running,
    }
  })

  return {
    endpoint: ref(value.endpoint),
    label: value.label as string,
    enabled: value.enabled,
    connection_state:
      value.connection_state as RuntimeEndpointConnectionState,
    capabilities:
      value.capabilities as unknown as RuntimeEndpoint['capabilities'],
    sessions,
  }
}

export function parseRuntimeEndpoints(value: unknown): RuntimeEndpoints {
  if (
    !record(value) ||
    value.adapter !== 'herdr' ||
    !Array.isArray(value.endpoints) ||
    value.endpoints.length > 65
  ) {
    throw Error('Invalid Herdr endpoint response')
  }

  const endpoints = value.endpoints.map(endpoint)
  const keys = endpoints.map((candidate) => endpointKey(candidate.endpoint))
  const localCount = keys.filter((key) => key === 'local').length
  if (
    new Set(keys).size !== keys.length ||
    (endpoints.length > 0 && localCount !== 1)
  ) {
    throw Error('Invalid endpoint identities')
  }
  return { adapter: 'herdr', endpoints }
}

function inventory(value: unknown): RuntimeInventory {
  if (
    !record(value) ||
    value.adapter !== 'herdr' ||
    !text(value.session) ||
    !record(value.focus) ||
    !Array.isArray(value.workspaces) ||
    !Array.isArray(value.tabs) ||
    !Array.isArray(value.panes) ||
    !Array.isArray(value.workers) ||
    !Array.isArray(value.child_agents)
  ) {
    throw Error('Invalid inventory')
  }
  return value as unknown as RuntimeInventory
}

export function parseMachineInventory(
  value: unknown,
  id: string,
): EndpointRuntimeInventory {
  if (!ID.test(id) || !record(value)) {
    throw Error('Invalid inventory')
  }
  const endpointRef = ref(value.endpoint)
  if (endpointRef.kind !== 'machine' || endpointRef.machine_id !== id) {
    throw Error('Remote inventory endpoint identity mismatch')
  }
  return { endpoint: endpointRef, inventory: inventory(value.inventory) }
}

function valid(value: string, name: string, optional = false) {
  if (optional && !value) {
    return
  }
  if (!value) {
    throw Error(`${name} is required.`)
  }
  if (value.includes('\0') || /[\r\n]/.test(value)) {
    throw Error(`${name} cannot contain newlines or NUL characters.`)
  }
  if ([...value].some(control)) {
    throw Error(`${name} cannot contain control characters.`)
  }
}

export const quotePosixArgument = (value: string) =>
  `'${value.replaceAll("'", `'"'"'`)}'`

export function buildAddMachineCommand({
  sshTarget,
  label,
  remoteSession,
}: {
  sshTarget: string
  label: string
  remoteSession: string
}) {
  valid(sshTarget, 'SSH target')
  valid(label, 'Label', true)
  valid(remoteSession, 'Remote session', true)
  return [
    'herdr machine add',
    label ? `--label=${quotePosixArgument(label)}` : '',
    remoteSession
      ? `--remote-session=${quotePosixArgument(remoteSession)}`
      : '',
    '--',
    quotePosixArgument(sshTarget),
  ]
    .filter(Boolean)
    .join(' ')
}

export function buildReconnectCommand(id: string) {
  if (!ID.test(id)) {
    throw Error('Reconnect requires a validated stable machine ID.')
  }
  return `herdr machine reconnect ${quotePosixArgument(id)}`
}

export function selectedEndpointAfterRefresh(
  key: string,
  endpoints: RuntimeEndpoint[],
) {
  return endpoints.some((candidate) => endpointKey(candidate.endpoint) === key)
    ? key
    : 'local'
}

export const CONNECTION_LABELS: Record<
  RuntimeEndpointConnectionState,
  string
> = {
  reachable: 'Connected',
  disabled: 'Disabled',
  authentication_required: 'Authentication required',
  unreachable: 'Unreachable',
  incompatible: 'Incompatible',
  unknown: 'Connection unknown',
}

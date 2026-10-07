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

const control = (character: string) => /\p{Cc}/u.test(character)

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

// Collection, identity, display, and protocol bounds mirror yard-herdr's
// machine/config boundaries. Herdr responses are capped at 8 MiB there; the
// remaining per-field limits keep hostile values well below that cap while a
// 16 KiB path remains larger than common platform absolute-path limits.
const MAX_INVENTORY_RECORDS = 10_000
const MAX_RUNTIME_ID_BYTES = 512
const MAX_DISPLAY_TEXT_BYTES = 128
const MAX_GENERIC_TEXT_BYTES = 64 * 1024
const MAX_PATH_BYTES = 16 * 1024
const MAX_ROLE_BYTES = 4 * 1024
const MAX_TOKEN_ENTRIES = 256
const MAX_TOKEN_NAME_BYTES = MAX_RUNTIME_ID_BYTES
const MAX_TOKEN_VALUE_BYTES = 8 * 1024
const MAX_U32 = 4_294_967_295
const MAX_U64 = 18_446_744_073_709_551_615n
const MAX_U64_DIGITS = 20
const U64 = /^(0|[1-9][0-9]*)$/

const STATUSES = new Set([
  'idle',
  'working',
  'blocked',
  'done',
  'unknown',
])

function boundedText(
  value: unknown,
  field: string,
  maximum: number,
  allowEmpty = false,
) {
  if (
    typeof value !== 'string' ||
    (!allowEmpty && value.length === 0) ||
    new TextEncoder().encode(value).length > maximum ||
    [...value].some(control)
  ) {
    throw Error(`Invalid ${field}`)
  }
  return value
}

const identity = (value: unknown, field: string) =>
  boundedText(value, field, MAX_RUNTIME_ID_BYTES)

const displayText = (value: unknown, field: string) =>
  boundedText(value, field, MAX_DISPLAY_TEXT_BYTES)

const genericText = (value: unknown, field: string) =>
  boundedText(value, field, MAX_GENERIC_TEXT_BYTES, true)

const pathText = (value: unknown, field: string) =>
  boundedText(value, field, MAX_PATH_BYTES)

function nullable<T>(
  value: unknown,
  field: string,
  parse: (value: unknown, field: string) => T,
): T | null {
  return value === null ? null : parse(value, field)
}

function optionalIdentity(value: unknown, field: string) {
  return value === undefined || value === null ? null : identity(value, field)
}

function boolean(value: unknown, field: string) {
  if (typeof value !== 'boolean') throw Error(`Invalid ${field}`)
  return value
}

function integer(value: unknown, field: string, maximum = Number.MAX_SAFE_INTEGER) {
  if (!Number.isSafeInteger(value) || (value as number) < 0 || (value as number) > maximum) {
    throw Error(`Invalid ${field}`)
  }
  return value as number
}

function count(value: unknown, field: string) {
  return integer(value, field, MAX_INVENTORY_RECORDS)
}

function status(value: unknown, field: string): RuntimeInventory['panes'][number]['status'] {
  if (typeof value !== 'string' || !STATUSES.has(value)) {
    throw Error(`Invalid ${field}`)
  }
  switch (value) {
    case 'idle':
    case 'working':
    case 'blocked':
    case 'done':
    case 'unknown':
      return value
    default:
      throw Error(`Invalid ${field}`)
  }
}

function u64(value: unknown, field: string) {
  if (
    typeof value !== 'string' ||
    value.length > MAX_U64_DIGITS ||
    !U64.test(value) ||
    BigInt(value) > MAX_U64
  ) {
    throw Error(`Invalid ${field}`)
  }
  return value
}

function collection(value: unknown, field: string) {
  if (!Array.isArray(value) || value.length > MAX_INVENTORY_RECORDS) {
    throw Error(`Invalid ${field}`)
  }
  return value
}

function tokens(value: unknown, field: string) {
  if (!record(value)) throw Error(`Invalid ${field}`)
  const entries = Object.entries(value)
  if (entries.length > MAX_TOKEN_ENTRIES) throw Error(`Invalid ${field}`)
  return Object.fromEntries(
    entries.map(([key, token]) => [
      boundedText(key, `${field} key`, MAX_TOKEN_NAME_BYTES),
      boundedText(token, `${field}.${key}`, MAX_TOKEN_VALUE_BYTES, true),
    ]),
  )
}

function providerSession(
  value: unknown,
  field: string,
): RuntimeInventory['panes'][number]['provider_session'] {
  if (!record(value)) throw Error(`Invalid ${field}`)
  return {
    source: identity(value.source, `${field}.source`),
    provider: identity(value.provider, `${field}.provider`),
    kind: identity(value.kind, `${field}.kind`),
    value: identity(value.value, `${field}.value`),
  }
}

function workspace(
  value: unknown,
  index: number,
): RuntimeInventory['workspaces'][number] {
  const field = `workspace[${index}]`
  if (!record(value)) throw Error(`Invalid ${field}`)
  const worktree = nullable(value.worktree, `${field}.worktree`, (entry, name) => {
    if (!record(entry)) throw Error(`Invalid ${name}`)
    return {
      repository_key: identity(entry.repository_key, `${name}.repository_key`),
      repository_name: displayText(entry.repository_name, `${name}.repository_name`),
      repository_root: pathText(entry.repository_root, `${name}.repository_root`),
      checkout_path: pathText(entry.checkout_path, `${name}.checkout_path`),
      is_linked: boolean(entry.is_linked, `${name}.is_linked`),
    }
  })
  return {
    runtime_id: identity(value.runtime_id, `${field}.runtime_id`),
    order: integer(value.order, `${field}.order`),
    label: displayText(value.label, `${field}.label`),
    focused: boolean(value.focused, `${field}.focused`),
    active_tab_id: identity(value.active_tab_id, `${field}.active_tab_id`),
    pane_count: count(value.pane_count, `${field}.pane_count`),
    tab_count: count(value.tab_count, `${field}.tab_count`),
    status: status(value.status, `${field}.status`),
    tokens: tokens(value.tokens, `${field}.tokens`),
    worktree,
  }
}

function tab(value: unknown, index: number): RuntimeInventory['tabs'][number] {
  const field = `tab[${index}]`
  if (!record(value)) throw Error(`Invalid ${field}`)
  return {
    runtime_id: identity(value.runtime_id, `${field}.runtime_id`),
    workspace_id: identity(value.workspace_id, `${field}.workspace_id`),
    order: integer(value.order, `${field}.order`),
    label: displayText(value.label, `${field}.label`),
    focused: boolean(value.focused, `${field}.focused`),
    pane_count: count(value.pane_count, `${field}.pane_count`),
    status: status(value.status, `${field}.status`),
  }
}

function pane(value: unknown, index: number): RuntimeInventory['panes'][number] {
  const field = `pane[${index}]`
  if (!record(value)) throw Error(`Invalid ${field}`)
  return {
    runtime_id: identity(value.runtime_id, `${field}.runtime_id`),
    pane_instance_id: optionalIdentity(value.pane_instance_id, `${field}.pane_instance_id`),
    terminal_id: identity(value.terminal_id, `${field}.terminal_id`),
    workspace_id: identity(value.workspace_id, `${field}.workspace_id`),
    tab_id: identity(value.tab_id, `${field}.tab_id`),
    focused: boolean(value.focused, `${field}.focused`),
    cwd: nullable(value.cwd, `${field}.cwd`, pathText),
    foreground_cwd: nullable(value.foreground_cwd, `${field}.foreground_cwd`, pathText),
    label: nullable(value.label, `${field}.label`, displayText),
    provider: nullable(value.provider, `${field}.provider`, identity),
    display_provider: nullable(value.display_provider, `${field}.display_provider`, displayText),
    status: status(value.status, `${field}.status`),
    tokens: tokens(value.tokens, `${field}.tokens`),
    provider_session: nullable(value.provider_session, `${field}.provider_session`, providerSession),
    revision: u64(value.revision, `${field}.revision`),
  }
}

function worker(value: unknown, index: number): RuntimeInventory['workers'][number] {
  const field = `worker[${index}]`
  if (!record(value)) throw Error(`Invalid ${field}`)
  return {
    runtime_id: identity(value.runtime_id, `${field}.runtime_id`),
    pane_instance_id: optionalIdentity(value.pane_instance_id, `${field}.pane_instance_id`),
    terminal_id: identity(value.terminal_id, `${field}.terminal_id`),
    workspace_id: identity(value.workspace_id, `${field}.workspace_id`),
    tab_id: identity(value.tab_id, `${field}.tab_id`),
    pane_id: identity(value.pane_id, `${field}.pane_id`),
    name: nullable(value.name, `${field}.name`, displayText),
    provider: nullable(value.provider, `${field}.provider`, identity),
    display_provider: nullable(value.display_provider, `${field}.display_provider`, displayText),
    status: status(value.status, `${field}.status`),
    focused: boolean(value.focused, `${field}.focused`),
    launch_pending: boolean(value.launch_pending, `${field}.launch_pending`),
    interactive_ready: boolean(value.interactive_ready, `${field}.interactive_ready`),
    state_change_sequence: u64(value.state_change_sequence, `${field}.state_change_sequence`),
    cwd: nullable(value.cwd, `${field}.cwd`, pathText),
    foreground_cwd: nullable(value.foreground_cwd, `${field}.foreground_cwd`, pathText),
    tokens: tokens(value.tokens, `${field}.tokens`),
    provider_session: nullable(value.provider_session, `${field}.provider_session`, providerSession),
    revision: u64(value.revision, `${field}.revision`),
  }
}

function childAgent(
  value: unknown,
  index: number,
): RuntimeInventory['child_agents'][number] {
  const field = `child_agent[${index}]`
  if (!record(value)) throw Error(`Invalid ${field}`)
  const parent = providerSession(value.parent_provider_session, `${field}.parent_provider_session`)
  if (parent === null) throw Error(`Invalid ${field}.parent_provider_session`)
  return {
    runtime_id: identity(value.runtime_id, `${field}.runtime_id`),
    parent_provider_session: parent,
    parent_agent_id: nullable(value.parent_agent_id, `${field}.parent_agent_id`, identity),
    provider: identity(value.provider, `${field}.provider`),
    provider_agent_id: identity(value.provider_agent_id, `${field}.provider_agent_id`),
    name: nullable(value.name, `${field}.name`, displayText),
    description: nullable(value.description, `${field}.description`, genericText),
    role: nullable(value.role, `${field}.role`, (entry, name) =>
      boundedText(entry, name, MAX_ROLE_BYTES, true)),
    status: status(value.status, `${field}.status`),
    depth: integer(value.depth, `${field}.depth`, MAX_U32),
    updated_at_unix_ms: integer(value.updated_at_unix_ms, `${field}.updated_at_unix_ms`),
  }
}

function uniqueIds(values: string[], field: string) {
  if (new Set(values).size !== values.length) throw Error(`Duplicate ${field}`)
}

function validateTopology(result: RuntimeInventory) {
  const workspaces = new Map(result.workspaces.map((entry) => [entry.runtime_id, entry]))
  const tabs = new Map(result.tabs.map((entry) => [entry.runtime_id, entry]))
  const panes = new Map(result.panes.map((entry) => [entry.runtime_id, entry]))

  uniqueIds(result.workspaces.map((entry) => entry.runtime_id), 'workspace identity')
  uniqueIds(result.tabs.map((entry) => entry.runtime_id), 'tab identity')
  uniqueIds(result.panes.map((entry) => entry.runtime_id), 'pane identity')
  uniqueIds(result.panes.map((entry) => entry.terminal_id), 'pane terminal identity')
  uniqueIds(result.workers.map((entry) => entry.runtime_id), 'worker identity')
  uniqueIds(result.workers.map((entry) => entry.terminal_id), 'worker terminal identity')
  uniqueIds(result.child_agents.map((entry) => entry.runtime_id), 'child agent identity')

  result.workspaces.forEach((entry) => {
    const activeTab = tabs.get(entry.active_tab_id)
    if (!activeTab || activeTab.workspace_id !== entry.runtime_id) {
      throw Error('Invalid workspace active tab ancestry')
    }
  })
  result.tabs.forEach((entry) => {
    if (!workspaces.has(entry.workspace_id)) throw Error('Invalid tab ancestry')
  })
  result.panes.forEach((entry) => {
    const parent = tabs.get(entry.tab_id)
    if (!workspaces.has(entry.workspace_id) || parent?.workspace_id !== entry.workspace_id) {
      throw Error('Invalid pane ancestry')
    }
  })
  result.workers.forEach((entry) => {
    const parent = panes.get(entry.pane_id)
    if (
      !parent ||
      entry.runtime_id !== entry.terminal_id ||
      parent.terminal_id !== entry.terminal_id ||
      parent.workspace_id !== entry.workspace_id ||
      parent.tab_id !== entry.tab_id ||
      parent.pane_instance_id !== entry.pane_instance_id
    ) {
      throw Error('Invalid worker ancestry')
    }
  })

  const focusedWorkspace = result.focus.workspace_id === null
    ? null
    : workspaces.get(result.focus.workspace_id)
  const focusedTab = result.focus.tab_id === null
    ? null
    : tabs.get(result.focus.tab_id)
  const focusedPane = result.focus.pane_id === null
    ? null
    : panes.get(result.focus.pane_id)
  if (
    (result.focus.workspace_id !== null && !focusedWorkspace) ||
    (result.focus.tab_id !== null && !focusedTab) ||
    (result.focus.pane_id !== null && !focusedPane) ||
    (focusedWorkspace && focusedTab && focusedTab.workspace_id !== focusedWorkspace.runtime_id) ||
    (focusedTab && focusedPane && focusedPane.tab_id !== focusedTab.runtime_id) ||
    (focusedWorkspace && focusedPane && focusedPane.workspace_id !== focusedWorkspace.runtime_id)
  ) {
    throw Error('Invalid focus ancestry')
  }
}

function inventory(value: unknown): RuntimeInventory {
  if (!record(value) || value.adapter !== 'herdr' || !record(value.focus)) {
    throw Error('Invalid inventory')
  }
  const result: RuntimeInventory = {
    adapter: 'herdr',
    session: displayText(value.session, 'inventory.session'),
    runtime_version: displayText(value.runtime_version, 'inventory.runtime_version'),
    protocol: integer(value.protocol, 'inventory.protocol', MAX_U32),
    observed_at_unix_ms: integer(value.observed_at_unix_ms, 'inventory.observed_at_unix_ms'),
    focus: {
      workspace_id: nullable(value.focus.workspace_id, 'inventory.focus.workspace_id', identity),
      tab_id: nullable(value.focus.tab_id, 'inventory.focus.tab_id', identity),
      pane_id: nullable(value.focus.pane_id, 'inventory.focus.pane_id', identity),
    },
    workspaces: collection(value.workspaces, 'inventory.workspaces').map(workspace),
    tabs: collection(value.tabs, 'inventory.tabs').map(tab),
    panes: collection(value.panes, 'inventory.panes').map(pane),
    workers: collection(value.workers, 'inventory.workers').map(worker),
    child_agents: collection(value.child_agents, 'inventory.child_agents').map(childAgent),
  }
  if (result.protocol < 19 || result.protocol > 22) {
    throw Error('Invalid inventory.protocol')
  }
  validateTopology(result)
  return result
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

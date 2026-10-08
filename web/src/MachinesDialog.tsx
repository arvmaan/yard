import {
  Check,
  ChevronRight,
  CircleAlert,
  Clipboard,
  Laptop,
  LoaderCircle,
  Plus,
  RefreshCw,
  Server,
  X,
} from 'lucide-react'
import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type FormEvent,
} from 'react'
import { createPortal } from 'react-dom'
import {
  hasDesktopMachineHandoff,
  launchMachineAdd,
  launchMachineReconnect,
} from './desktopBridge'
import { fetchMachineInventory, fetchRuntimeEndpoints } from './api'
import {
  buildAddMachineCommand,
  buildReconnectCommand,
  CONNECTION_LABELS,
  endpointKey,
  parseMachineInventory,
  parseRuntimeEndpoints,
  selectedEndpointAfterRefresh,
} from './remoteMachines'
import type { RuntimeEndpoint, RuntimeInventory } from './types'
import { useModalDialog } from './useModalDialog'

type RemoteState = {
  error: string | null
  inventory: RuntimeInventory | null
  loading: boolean
  stale: boolean
}

const session = (endpoint: RuntimeEndpoint) =>
  endpoint.sessions.find((candidate) => candidate.is_default)?.name ??
  endpoint.sessions[0]?.name ??
  'Not reported'

const capabilities = (endpoint: RuntimeEndpoint) =>
  [
    ['inventory', endpoint.capabilities.inventory_read],
    ['management', endpoint.capabilities.mutations],
    ['terminal', endpoint.capabilities.terminal_streaming],
  ]
    .filter((candidate) => candidate[1])
    .map((candidate) => candidate[0])
    .join(', ') || 'none available'

const copy = (value: string) => navigator.clipboard.writeText(value)

function Topology({ inventory }: { inventory: RuntimeInventory }) {
  const tabs = useMemo(() => {
    const grouped = new Map<string, typeof inventory.tabs>()
    inventory.tabs.forEach((tab) =>
      grouped.set(tab.workspace_id, [
        ...(grouped.get(tab.workspace_id) ?? []),
        tab,
      ]),
    )
    return grouped
  }, [inventory.tabs])
  const panes = useMemo(() => {
    const grouped = new Map<string, typeof inventory.panes>()
    inventory.panes.forEach((pane) =>
      grouped.set(pane.tab_id, [
        ...(grouped.get(pane.tab_id) ?? []),
        pane,
      ]),
    )
    return grouped
  }, [inventory.panes])
  const workers = useMemo(() => {
    const grouped = new Map<string, typeof inventory.workers>()
    inventory.workers.forEach((worker) =>
      grouped.set(worker.pane_id, [
        ...(grouped.get(worker.pane_id) ?? []),
        worker,
      ]),
    )
    return grouped
  }, [inventory.workers])

  return (
    <div className="machines-dialog__topology">
      {inventory.workspaces.map((workspace) => (
        <section key={workspace.runtime_id}>
          <h4>{workspace.label}</h4>
          <code>{workspace.runtime_id}</code>
          {(tabs.get(workspace.runtime_id) ?? []).map((tab) => (
            <div className="machines-dialog__tab" key={tab.runtime_id}>
              <strong>{tab.label}</strong>
              <code>{tab.runtime_id}</code>
              {(panes.get(tab.runtime_id) ?? []).map((pane) => (
                <div className="machines-dialog__pane" key={pane.runtime_id}>
                  <span>{pane.label ?? 'Pane'}</span>
                  <code>{pane.runtime_id}</code>
                  {(workers.get(pane.runtime_id) ?? []).map((worker) => (
                    <small key={worker.runtime_id}>
                      Observed agent:{' '}
                      {worker.name ??
                        worker.display_provider ??
                        worker.provider ??
                        'Unnamed'}{' '}
                      · {worker.status}
                    </small>
                  ))}
                </div>
              ))}
            </div>
          ))}
        </section>
      ))}
      {inventory.workspaces.length === 0 ? (
        <p className="empty-state">
          No workspaces were observed on this machine.
        </p>
      ) : null}
    </div>
  )
}

function AddMachineDialog({
  onClose,
  returnFocus,
}: {
  onClose: () => void
  returnFocus: HTMLElement | null
}) {
  const title = useId()
  const dialog = useRef<HTMLElement>(null)
  const targetRef = useRef<HTMLInputElement>(null)
  const [target, setTarget] = useState('')
  const [label, setLabel] = useState('')
  const [remoteSession, setRemoteSession] = useState('')
  const [command, setCommand] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)
  const pendingRef = useRef(false)
  const [pending, setPending] = useState(false)
  const [launched, setLaunched] = useState(false)
  const desktop = hasDesktopMachineHandoff()
  const close = useModalDialog({
    dialogRef: dialog,
    initialFocusRef: targetRef,
    onClose,
    returnFocus,
  })

  const submit = async (event: FormEvent) => {
    event.preventDefault()
    if (pendingRef.current) return
    try {
      const nextCommand = buildAddMachineCommand({
        sshTarget: target,
        label,
        remoteSession,
      })
      setError(null)
      setCopied(false)
      setLaunched(false)
      if (!desktop) {
        setCommand(nextCommand)
        return
      }
      setCommand(null)
      pendingRef.current = true
      setPending(true)
      await launchMachineAdd({
        ssh_target: target,
        ...(label.trim() ? { label } : {}),
        ...(remoteSession.trim() ? { session: remoteSession } : {}),
      })
      setLaunched(true)
    } catch (caught) {
      setCommand(null)
      setError(
        desktop
          ? 'Terminal could not be opened. Try again, or use Yard in a browser to copy the command.'
          : caught instanceof Error
            ? caught.message
            : 'Invalid machine details.',
      )
    } finally {
      pendingRef.current = false
      setPending(false)
    }
  }

  return createPortal(
    <div
      className="modal-backdrop machines-dialog__nested-backdrop"
      role="presentation"
    >
      <section
        aria-labelledby={title}
        aria-modal="true"
        className="control-dialog add-machine-dialog"
        ref={dialog}
        role="dialog"
      >
        <header className="machines-dialog__header">
          <div>
            <p className="eyebrow">Herdr setup handoff</p>
            <h2 id={title}>Add saved machine</h2>
          </div>
          <button
            aria-label="Close add machine"
            className="icon-button"
            onClick={close}
            type="button"
          >
            <X aria-hidden="true" size={16} />
          </button>
        </header>
        <form className="dialog-form" onSubmit={submit}>
          <p className="machines-dialog__guidance">
            Yard does not collect credentials or edit Herdr configuration.
            {desktop
              ? ' Terminal opens visibly so Herdr and SSH can prompt you directly.'
              : ' Enter only connection metadata to prepare a command for a trusted terminal.'}
          </p>
          <label>
            <span>SSH target</span>
            <input
              autoComplete="off"
              onChange={(event) => setTarget(event.target.value)}
              placeholder="user@host"
              ref={targetRef}
              value={target}
            />
          </label>
          <label>
            <span>Label (optional)</span>
            <input
              autoComplete="off"
              onChange={(event) => setLabel(event.target.value)}
              value={label}
            />
          </label>
          <label>
            <span>Remote session (optional)</span>
            <input
              autoComplete="off"
              onChange={(event) => setRemoteSession(event.target.value)}
              value={remoteSession}
            />
          </label>
          {error ? (
            <p className="dialog-error" role="alert">
              {error}
            </p>
          ) : null}
          {launched ? (
            <p className="machines-dialog__guidance" role="status">
              Terminal opened. Complete Herdr setup there, then close this
              dialog and Refresh saved machines.
            </p>
          ) : null}
          {command ? (
            <section
              aria-label="Prepared Herdr command"
              className="machines-dialog__command"
            >
              <strong>Command for a trusted POSIX terminal</strong>
              <code>{command}</code>
              <p>
                Review it before running. Herdr and SSH remain responsible for
                known-host verification and authentication; copying does not
                execute the command.
              </p>
              <button
                className="secondary-button"
                onClick={() =>
                  void copy(command)
                    .then(() => setCopied(true))
                    .catch(() =>
                      setError(
                        'Could not copy the command. Select it manually.',
                      ),
                    )
                }
                type="button"
              >
                {copied ? (
                  <Check aria-hidden="true" size={14} />
                ) : (
                  <Clipboard aria-hidden="true" size={14} />
                )}{' '}
                {copied ? 'Copied' : 'Copy command'}
              </button>
            </section>
          ) : null}
          <footer className="dialog-actions">
            <button
              className="secondary-button"
              onClick={close}
              type="button"
            >
              Cancel
            </button>
            <button className="primary-button" disabled={pending} type="submit">
              {pending ? (
                <LoaderCircle aria-hidden="true" className="status-spin" size={14} />
              ) : null}{' '}
              {desktop ? 'Open setup in Terminal' : 'Prepare command'}
            </button>
          </footer>
        </form>
      </section>
    </div>,
    document.body,
  )
}

export function MachinesDialog({
  onClose,
  returnFocus,
}: {
  onClose: () => void
  returnFocus: HTMLElement | null
}) {
  const title = useId()
  const dialog = useRef<HTMLElement>(null)
  const addButton = useRef<HTMLButtonElement>(null)
  const controller = useRef<AbortController | null>(null)
  const [endpoints, setEndpoints] = useState<RuntimeEndpoint[]>([])
  const [selectedKey, setSelectedKey] = useState('local')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [addOpen, setAddOpen] = useState(false)
  const [remote, setRemote] = useState<Record<string, RemoteState>>({})
  const [copied, setCopied] = useState(false)
  const reconnectPendingRef = useRef(false)
  const [reconnectPending, setReconnectPending] = useState(false)
  const [reconnectLaunched, setReconnectLaunched] = useState(false)
  const [reconnectError, setReconnectError] = useState<string | null>(null)
  const desktop = hasDesktopMachineHandoff()
  const close = useModalDialog({
    active: !addOpen,
    dialogRef: dialog,
    onClose,
    returnFocus,
  })

  const load = useCallback(async () => {
    controller.current?.abort()
    const nextController = new AbortController()
    controller.current = nextController
    setLoading(true)
    try {
      const parsed = parseRuntimeEndpoints(
        await fetchRuntimeEndpoints(nextController.signal),
      )
      if (nextController.signal.aborted) {
        return
      }
      setEndpoints(parsed.endpoints)
      setSelectedKey((key) =>
        selectedEndpointAfterRefresh(key, parsed.endpoints),
      )
      setError(null)
    } catch {
      if (!nextController.signal.aborted) {
        setError(
          'Saved machines could not be refreshed. Existing observations are unchanged.',
        )
      }
    } finally {
      if (controller.current === nextController) {
        controller.current = null
        setLoading(false)
      }
    }
  }, [])

  useEffect(() => {
    void load()
    return () => controller.current?.abort()
  }, [load])

  const selected =
    endpoints.find(
      (candidate) => endpointKey(candidate.endpoint) === selectedKey,
    ) ?? null
  const id =
    selected?.endpoint.kind === 'machine'
      ? selected.endpoint.machine_id
      : null
  const state = id ? remote[id] : null

  const loadRemote = useCallback(async (endpoint: RuntimeEndpoint) => {
    if (endpoint.endpoint.kind !== 'machine') {
      return
    }
    const id = endpoint.endpoint.machine_id
    setRemote((current) => ({
      ...current,
      [id]: {
        error: null,
        inventory: current[id]?.inventory ?? null,
        loading: true,
        stale: current[id]?.stale ?? false,
      },
    }))
    try {
      const result = parseMachineInventory(
        await fetchMachineInventory(id),
        id,
      )
      setRemote((current) => ({
        ...current,
        [id]: {
          error: null,
          inventory: result.inventory,
          loading: false,
          stale: false,
        },
      }))
    } catch {
      setRemote((current) => ({
        ...current,
        [id]: {
          error:
            'Remote inventory could not be loaded. Yard did not fall back to Local.',
          inventory: current[id]?.inventory ?? null,
          loading: false,
          stale: Boolean(current[id]?.inventory),
        },
      }))
    }
  }, [])

  const canInspect = Boolean(
    selected?.endpoint.kind === 'machine' &&
      selected.enabled &&
      selected.connection_state === 'reachable' &&
      selected.capabilities.inventory_read,
  )

  useEffect(() => {
    if (
      selected?.endpoint.kind === 'machine' &&
      canInspect &&
      !remote[selected.endpoint.machine_id]
    ) {
      void loadRemote(selected)
    }
  }, [canInspect, loadRemote, remote, selected])

  return createPortal(
    <div
      className="modal-backdrop machines-dialog__backdrop"
      role="presentation"
    >
      <section
        aria-labelledby={title}
        aria-modal="true"
        className="machines-dialog"
        id="machines-dialog"
        ref={dialog}
        role="dialog"
      >
        <header className="machines-dialog__header">
          <div>
            <p className="eyebrow">Herdr endpoints</p>
            <h2 id={title}>Machines</h2>
          </div>
          <button
            aria-label="Close machines"
            className="icon-button"
            onClick={close}
            type="button"
          >
            <X aria-hidden="true" size={16} />
          </button>
        </header>
        <div className="machines-dialog__toolbar">
          <button
            className="secondary-button"
            onClick={() => setAddOpen(true)}
            ref={addButton}
            type="button"
          >
            <Plus aria-hidden="true" size={14} /> Add machine
          </button>
          <button
            aria-label="Refresh saved machines"
            className="secondary-button"
            disabled={loading}
            onClick={() => void load()}
            type="button"
          >
            <RefreshCw
              aria-hidden="true"
              className={loading ? 'status-spin' : ''}
              size={14}
            />{' '}
            Refresh
          </button>
        </div>
        {error ? (
          <p className="machines-dialog__error" role="alert">
            <CircleAlert aria-hidden="true" size={14} /> {error}
          </p>
        ) : null}
        <div className="machines-dialog__body">
          <nav aria-label="Herdr machines" className="machines-dialog__list">
            {endpoints.map((endpoint) => {
              const key = endpointKey(endpoint.endpoint)
              return (
                <button
                  aria-pressed={selectedKey === key}
                  className="machines-dialog__machine"
                  key={key}
                  onClick={() => {
                    setSelectedKey(key)
                    setCopied(false)
                    setReconnectLaunched(false)
                    setReconnectError(null)
                  }}
                  type="button"
                >
                  {endpoint.endpoint.kind === 'local' ? (
                    <Laptop aria-hidden="true" size={15} />
                  ) : (
                    <Server aria-hidden="true" size={15} />
                  )}
                  <span>
                    <strong>{endpoint.label}</strong>
                    <small>
                      {session(endpoint)} ·{' '}
                      {endpoint.enabled
                        ? CONNECTION_LABELS[endpoint.connection_state]
                        : 'Disabled'}
                    </small>
                  </span>
                  <ChevronRight aria-hidden="true" size={14} />
                </button>
              )
            })}
            {loading && endpoints.length === 0 ? (
              <p className="empty-state">
                <LoaderCircle
                  aria-hidden="true"
                  className="status-spin"
                  size={15}
                />{' '}
                Loading saved machines…
              </p>
            ) : null}
            {!loading && endpoints.length === 0 && !error ? (
              <p className="empty-state">
                No saved endpoints were returned by Herdr.
              </p>
            ) : null}
          </nav>
          <article className="machines-dialog__details">
            {selected ? (
              <>
                <p className="eyebrow">
                  {selected.endpoint.kind === 'local'
                    ? 'Durable Yard endpoint'
                    : 'Observed endpoint'}
                </p>
                <h3>{selected.label}</h3>
                <dl>
                  <div>
                    <dt>Session</dt>
                    <dd>{session(selected)}</dd>
                  </div>
                  <div>
                    <dt>Status</dt>
                    <dd>{CONNECTION_LABELS[selected.connection_state]}</dd>
                  </div>
                  <div>
                    <dt>Enabled</dt>
                    <dd>{selected.enabled ? 'Yes' : 'No'}</dd>
                  </div>
                  <div>
                    <dt>Capabilities</dt>
                    <dd>{capabilities(selected)}</dd>
                  </div>
                </dl>
                {selected.endpoint.kind === 'local' ? (
                  <p className="machines-dialog__guidance">
                    Local remains the active durable Yard runtime. Machine
                    inspection does not change the selected local session or
                    fleet.
                  </p>
                ) : (
                  <>
                    <p className="machines-dialog__readonly">
                      <strong>Observed / read-only.</strong> Remote panes and
                      agents are not durable Yard workers or project members.
                      Yard provides no remote controls in this view.
                    </p>
                    {!canInspect && id ? (
                      <section className="machines-dialog__guidance">
                        <strong>Inventory unavailable</strong>
                        <p>
                          {selected.enabled
                            ? 'Refresh after resolving the connection in Herdr.'
                            : 'This saved profile is disabled. Enable it with Herdr, then refresh.'}
                        </p>
                        <div className="machines-dialog__command">
                          {!desktop ? <code>{buildReconnectCommand(id)}</code> : null}
                          <p>
                            {desktop
                              ? 'Terminal opens visibly for SSH authentication. After reconnecting, Refresh saved machines.'
                              : 'Run this in a trusted terminal to complete SSH authentication. Copying does not execute it.'}
                          </p>
                          {reconnectError ? (
                            <p className="dialog-error" role="alert">
                              {reconnectError}
                            </p>
                          ) : null}
                          {reconnectLaunched ? (
                            <p role="status">
                              Terminal opened. Complete authentication there,
                              then Refresh saved machines.
                            </p>
                          ) : null}
                          <button
                            className="secondary-button"
                            disabled={reconnectPending}
                            onClick={() => {
                              if (reconnectPendingRef.current) return
                              if (!desktop) {
                                void copy(buildReconnectCommand(id))
                                  .then(() => setCopied(true))
                                  .catch(() => undefined)
                                return
                              }
                              reconnectPendingRef.current = true
                              setReconnectPending(true)
                              setReconnectError(null)
                              setReconnectLaunched(false)
                              void launchMachineReconnect({ machine_id: id })
                                .then(() => setReconnectLaunched(true))
                                .catch(() =>
                                  setReconnectError(
                                    'Terminal could not be opened. Try again, or use Yard in a browser to copy the command.',
                                  ),
                                )
                                .finally(() => {
                                  reconnectPendingRef.current = false
                                  setReconnectPending(false)
                                })
                            }}
                            type="button"
                          >
                            {desktop ? (
                              reconnectPending ? (
                                <LoaderCircle
                                  aria-hidden="true"
                                  className="status-spin"
                                  size={14}
                                />
                              ) : null
                            ) : (
                              <Clipboard aria-hidden="true" size={14} />
                            )}{' '}
                            {desktop
                              ? reconnectPending
                                ? 'Opening Terminal…'
                                : 'Open Terminal to reconnect'
                              : copied
                                ? 'Copied'
                                : 'Copy reconnect command'}
                          </button>
                        </div>
                      </section>
                    ) : null}
                    {canInspect && state?.loading ? (
                      <p className="empty-state">
                        <LoaderCircle
                          aria-hidden="true"
                          className="status-spin"
                          size={15}
                        />{' '}
                        Loading remote inventory…
                      </p>
                    ) : null}
                    {canInspect && state?.error ? (
                      <p className="machines-dialog__error" role="alert">
                        <CircleAlert aria-hidden="true" size={14} />{' '}
                        {state.error}
                      </p>
                    ) : null}
                    {canInspect && state?.inventory ? (
                      <>
                        {state.stale ? (
                          <p className="machines-dialog__error">
                            Showing the last observed remote snapshot.
                          </p>
                        ) : null}
                        <button
                          className="secondary-button"
                          disabled={state.loading}
                          onClick={() => void loadRemote(selected)}
                          type="button"
                        >
                          <RefreshCw aria-hidden="true" size={14} /> Refresh
                          inventory
                        </button>
                        <Topology inventory={state.inventory} />
                      </>
                    ) : null}
                  </>
                )}
              </>
            ) : (
              <p className="empty-state">Select a machine.</p>
            )}
          </article>
        </div>
      </section>
      {addOpen ? (
        <AddMachineDialog
          onClose={() => setAddOpen(false)}
          returnFocus={addButton.current}
        />
      ) : null}
    </div>,
    document.body,
  )
}

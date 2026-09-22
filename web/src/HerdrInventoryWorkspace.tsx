import {
  ArrowLeft,
  Bot,
  CircleAlert,
  RefreshCw,
  Search,
  Server,
  X,
} from 'lucide-react'
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from 'react'
import { createPortal } from 'react-dom'
import { YardApiError, fetchHerdrFleetInventory } from './api'
import {
  clearHerdrInventoryFailures,
  countHerdrInventory,
  filterHerdrFailures,
  filterHerdrInventory,
  herdrInventoryStatus,
  herdrPaneAccessibleDescription,
  herdrPaneAccessibleName,
  herdrPaneClassificationLabel,
  herdrPaneLabel,
  herdrPaneStatusLabel,
  herdrRequestFailureStatus,
  mergeHerdrInventorySnapshot,
  safeHerdrFailureReason,
  safeHerdrPaneMetadataReason,
  safeHerdrPaneReason,
  safeHerdrRequestError,
} from './herdrInventory'
import type { HerdrFleetInventory, HerdrLivePane } from './types'
import { useModalDialog } from './useModalDialog'

interface HerdrInventoryWorkspaceProps {
  onClose: () => void
}

interface InventoryState {
  inventory: HerdrFleetInventory | null
  staleAll: boolean
  staleSessions: string[]
}

interface FleetFailureCounts {
  attemptedSessions: number
  failedSessions: number
}


function DetailRow({ label, value }: { label: string; value: string | null }) {
  if (!value) return null
  return (
    <div className="herdr-inventory__detail-row">
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  )
}

export function HerdrInventoryWorkspace({ onClose }: HerdrInventoryWorkspaceProps) {
  const [snapshot, setSnapshot] = useState<InventoryState>({
    inventory: null,
    staleAll: false,
    staleSessions: [],
  })
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [query, setQuery] = useState('')
  const [selectedKey, setSelectedKey] = useState<string | null>(null)
  const [activeKey, setActiveKey] = useState<string | null>(null)
  const [requestFailureCounts, setRequestFailureCounts] =
    useState<FleetFailureCounts | null>(null)
  const [focusNotice, setFocusNotice] = useState<string | null>(null)
  const dialogRef = useRef<HTMLDivElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)
  const detailsRef = useRef<HTMLElement>(null)
  const returnRef = useRef<HTMLButtonElement>(null)
  const rowRefs = useRef(new Map<string, HTMLButtonElement>())
  const detailsFocusKey = useRef<string | null>(null)
  const activeIndex = useRef(0)
  const focusedRowKey = useRef<string | null>(null)
  const focusRecoveryKey = useRef<string | null>(null)
  const refreshController = useRef<AbortController | null>(null)
  const inventory = snapshot.inventory
  const staleSessions = useMemo(
    () => new Set(snapshot.staleSessions),
    [snapshot.staleSessions],
  )



  const cancelRefresh = useCallback(() => {
    refreshController.current?.abort()
    refreshController.current = null
  }, [])

  const closeInventory = useCallback(() => {
    cancelRefresh()
    onClose()
  }, [cancelRefresh, onClose])

  const load = useCallback(async () => {
    refreshController.current?.abort()
    setFocusNotice(null)
    const controller = new AbortController()
    refreshController.current = controller
    setLoading(true)
    try {
      const next = await fetchHerdrFleetInventory(controller.signal)
      if (refreshController.current !== controller || controller.signal.aborted) return
      focusRecoveryKey.current = focusedRowKey.current
      setSnapshot((current) => {
        const merged = mergeHerdrInventorySnapshot(current.inventory, next)
        return { ...merged, staleAll: false }
      })
      setRequestFailureCounts(null)
      setError(null)
    } catch (caught) {
      if (refreshController.current !== controller || controller.signal.aborted) return
      setSnapshot((current) => ({
        ...current,
        inventory: clearHerdrInventoryFailures(current.inventory),
        staleAll: current.inventory !== null,
        staleSessions: current.inventory?.sessions.map(({ id }) => id) ?? [],
      }))
      setRequestFailureCounts(
        caught instanceof YardApiError &&
          caught.attemptedSessions !== null &&
          caught.attemptedSessions > 0 &&
          caught.failedSessions !== null
          ? {
              attemptedSessions: caught.attemptedSessions,
              failedSessions: caught.failedSessions,
            }
          : null,
      )
      setError(
        safeHerdrRequestError(
          caught instanceof YardApiError ? caught.code : undefined,
        ),
      )
    } finally {
      if (refreshController.current === controller) {
        refreshController.current = null
        setLoading(false)
      }
    }
  }, [])

  useEffect(() => {
    void load()
    return cancelRefresh
  }, [cancelRefresh, load])
  useModalDialog({
    dialogRef,
    initialFocusRef: searchRef,
    onClose: closeInventory,
  })

  const groups = useMemo(
    () =>
      inventory
        ? filterHerdrInventory(
            inventory,
            query,
            staleSessions,
            snapshot.staleAll,
          )
        : [],
    [inventory, query, snapshot.staleAll, staleSessions],
  )
  const visibleFailures = useMemo(
    () => (inventory ? filterHerdrFailures(inventory, query) : []),
    [inventory, query],
  )
  const visiblePanes = useMemo(
    () =>
      groups.flatMap((session) =>
        session.workspaces.flatMap((workspace) => workspace.panes),
      ),
    [groups],
  )
  const selected =
    visiblePanes.find((pane) => pane.identity_key === selectedKey) ?? null
  const selectedStale =
    selected !== null &&
    (snapshot.staleAll || staleSessions.has(selected.session))
  const selectedSessionLabel = selected
    ? groups.find((session) => session.id === selected.session)?.label ??
      selected.session
    : null
  const counts = useMemo(
    () =>
      inventory
        ? countHerdrInventory(
            inventory,
            groups,
            staleSessions,
            snapshot.staleAll,
          )
        : null,
    [groups, inventory, snapshot.staleAll, staleSessions],
  )
  const displayedCounts = counts
    ? {
        ...counts,
        failedSessions:
          requestFailureCounts?.failedSessions ?? counts.failedSessions,
      }
    : requestFailureCounts
      ? {
          freshTotal: 0,
          visibleTotal: 0,
          visibleAgents: 0,
          visibleRuntime: 0,
          visibleUnattached: 0,
          staleTotal: 0,
          failedSessions: requestFailureCounts.failedSessions,
        }
      : null
  const inventoryStatus = error
    ? requestFailureCounts
      ? herdrRequestFailureStatus(
          requestFailureCounts.attemptedSessions,
          requestFailureCounts.failedSessions,
        )
      : error
    : displayedCounts
      ? herdrInventoryStatus(displayedCounts, visibleFailures.length)
      : 'Observing Herdr inventory.'
  const liveStatus = focusNotice ?? inventoryStatus

  useEffect(() => {
    const selectedRemoved = Boolean(selectedKey && !selected)
    const activeVisible = Boolean(
      activeKey &&
        visiblePanes.some(({ identity_key }) => identity_key === activeKey),
    )
    const focusedRowRemoved = Boolean(
      activeKey &&
        (focusRecoveryKey.current === activeKey ||
          focusedRowKey.current === activeKey) &&
        !activeVisible,
    )

    if (selectedRemoved) {
      setSelectedKey(null)
      detailsFocusKey.current = null
    }
    if (visiblePanes.length === 0) {
      if (activeKey) setActiveKey(null)
      if (focusedRowRemoved) {
        searchRef.current?.focus()
        setFocusNotice(
          selectedRemoved
            ? 'Selected pane is no longer visible. Focus moved to search.'
            : 'Focused pane is no longer visible. Focus moved to search.',
        )
      } else if (selectedRemoved) {
        setFocusNotice('Selected pane is no longer visible.')
      }
      focusRecoveryKey.current = null
      return
    }
    if (!activeVisible) {
      const next = visiblePanes[Math.min(activeIndex.current, visiblePanes.length - 1)]
      activeIndex.current = Math.min(activeIndex.current, visiblePanes.length - 1)
      setActiveKey(next.identity_key)
      if (focusedRowRemoved) {
        rowRefs.current.get(next.identity_key)?.focus()
        setFocusNotice(
          `${selectedRemoved ? 'Selected' : 'Focused'} pane is no longer visible. Focus moved to ${herdrPaneLabel(next)}.`,
        )
      } else if (selectedRemoved) {
        setFocusNotice('Selected pane is no longer visible.')
      }
    } else if (selectedRemoved) {
      setFocusNotice('Selected pane is no longer visible.')
    }
    focusRecoveryKey.current = null
  }, [activeKey, selected, selectedKey, visiblePanes])

  useEffect(() => {
    if (!selected || detailsFocusKey.current !== selected.identity_key) return
    detailsFocusKey.current = null
    const frame = window.requestAnimationFrame(() => {
      ;(returnRef.current ?? detailsRef.current)?.focus()
    })
    return () => window.cancelAnimationFrame(frame)
  }, [selected])

  const focusRow = useCallback(
    (key: string) => {
      activeIndex.current = Math.max(
        0,
        visiblePanes.findIndex(({ identity_key }) => identity_key === key),
      )
      setActiveKey(key)
      rowRefs.current.get(key)?.focus()
    },
    [visiblePanes],
  )

  const selectPane = useCallback(
    (pane: HerdrLivePane) => {
      activeIndex.current = Math.max(
        0,
        visiblePanes.findIndex(({ identity_key }) => identity_key === pane.identity_key),
      )
      setFocusNotice(null)
      setActiveKey(pane.identity_key)
      if (selectedKey === pane.identity_key) {
        window.requestAnimationFrame(() =>
          (returnRef.current ?? detailsRef.current)?.focus(),
        )
        return
      }
      detailsFocusKey.current = pane.identity_key
      setSelectedKey(pane.identity_key)
    },
    [selectedKey, visiblePanes],
  )

  const handleRowKey = (
    event: ReactKeyboardEvent<HTMLButtonElement>,
    pane: HerdrLivePane,
  ) => {
    const index = visiblePanes.findIndex(
      ({ identity_key }) => identity_key === pane.identity_key,
    )
    let nextIndex: number | null = null
    if (event.key === 'ArrowDown') nextIndex = Math.min(index + 1, visiblePanes.length - 1)
    if (event.key === 'ArrowUp') nextIndex = Math.max(index - 1, 0)
    if (event.key === 'Home') nextIndex = 0
    if (event.key === 'End') nextIndex = visiblePanes.length - 1
    if (nextIndex !== null) {
      event.preventDefault()
      focusRow(visiblePanes[nextIndex].identity_key)
      return
    }
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault()
      selectPane(pane)
    }
  }

  return createPortal(
    <div
      aria-label="Herdr inventory"
      aria-modal="true"
      className="herdr-inventory-overlay"
      onFocusCapture={() => {
        focusedRowKey.current = null
      }}
      onKeyDown={(event) => {
        if (event.key !== 'Escape') return
        event.preventDefault()
        event.stopPropagation()
        closeInventory()
      }}
      ref={dialogRef}
      role="dialog"
    >
    <section
      aria-label="Live Herdr inventory"
      className="herdr-inventory"
      id="herdr-inventory-workspace"
    >
      <header className="herdr-inventory__header">
        <div>
          <p className="eyebrow">Operational inventory</p>
          <h1>Live Herdr panes</h1>
        </div>
        <span aria-label="Herdr inventory counts">
          {displayedCounts
            ? `${displayedCounts.freshTotal} fresh · ${displayedCounts.visibleTotal} shown · ${displayedCounts.visibleAgents} ${displayedCounts.visibleAgents === 1 ? 'agent' : 'agents'} · ${displayedCounts.visibleRuntime} runtime` +
              (displayedCounts.visibleUnattached
                ? ` · ${displayedCounts.visibleUnattached} unattached ${displayedCounts.visibleUnattached === 1 ? 'agent' : 'agents'}`
                : '') +
              ` · ${displayedCounts.staleTotal} stale · ${displayedCounts.failedSessions} failed ${displayedCounts.failedSessions === 1 ? 'session' : 'sessions'}`
            : 'Observing Herdr'}
        </span>
        <button
          aria-label="Refresh Herdr inventory"
          className="icon-button"
          disabled={loading}
          onClick={() => void load()}
          type="button"
        >
          <RefreshCw
            aria-hidden="true"
            className={loading ? 'status-spin' : ''}
            size={16}
          />
        </button>
        <button
          aria-label="Close Herdr inventory"
          className="icon-button"
          onClick={closeInventory}
          type="button"
        >
          <X aria-hidden="true" size={16} />
        </button>
      </header>
      <span
        aria-atomic="true"
        aria-live="polite"
        className="visually-hidden"
        role="status"
      >
        {liveStatus}
      </span>

      <label className="herdr-inventory__search">
        <Search aria-hidden="true" size={15} />
        <span className="visually-hidden">Search live Herdr panes</span>
        <input
          aria-label="Search live Herdr panes"
          onChange={(event) => {
            focusRecoveryKey.current = focusedRowKey.current
            setFocusNotice(null)
            setQuery(event.target.value)
          }}
          placeholder="Search sessions, workspaces, panes…"
          ref={searchRef}
          type="search"
          value={query}
        />
      </label>

      {error || visibleFailures.length ? (
        <div className="herdr-inventory__failures">
          {error ? (
            <p>
              <CircleAlert aria-hidden="true" size={14} />
              {error}
              {inventory ? ' Showing the last observed snapshot.' : ''}
              {requestFailureCounts
                ? ` ${herdrRequestFailureStatus(
                    requestFailureCounts.attemptedSessions,
                    requestFailureCounts.failedSessions,
                  )}`
                : ''}
            </p>
          ) : null}
          {visibleFailures.map((failure) => (
            <p key={failure.session}>
              <CircleAlert aria-hidden="true" size={14} />
              <strong>{failure.session}</strong>{' '}
              {safeHerdrFailureReason(failure.code)}
            </p>
          ))}
        </div>
      ) : null}

      <div className="herdr-inventory__body">
        <nav aria-label="Running Herdr sessions" className="herdr-inventory__list">
          {groups.map((session, sessionIndex) => {
            const sessionStale = snapshot.staleAll || staleSessions.has(session.id)
            return (
              <section
                className="herdr-inventory__session"
                data-stale={sessionStale}
                key={session.id}
              >
                <h2>
                  <span>
                    {session.label}
                    {session.isDefault ? ' · default' : ''}
                    {session.metadataReason ? ' · ambiguous' : ''}
                    {sessionStale ? ' · stale' : ''}
                  </span>
                  <small>
                    {session.panes}
                    {session.unattached ? ` · ${session.unattached} unattached` : ''}
                  </small>
                </h2>
                {session.metadataReason ? (
                  <p className="herdr-inventory__state" data-observation="ambiguous">
                    {session.metadataReason}
                  </p>
                ) : null}
                {session.workspaces.map((workspace, workspaceIndex) => (
                  <div className="herdr-inventory__workspace" key={workspace.id}>
                    <h3>
                      <span>{workspace.label}</span>
                      <small>
                        {workspace.paneCount}
                        {workspace.unattachedCount
                          ? ` · ${workspace.unattachedCount} unattached`
                          : ''}
                      </small>
                    </h3>
                    {workspace.panes.map((pane, paneIndex) => {
                      const Icon = pane.kind === 'agent' ? Bot : Server
                      const label = herdrPaneLabel(pane)
                      const classification = herdrPaneClassificationLabel(pane)
                      const status = herdrPaneStatusLabel(pane, sessionStale)
                      const descriptionId = `herdr-pane-description-${sessionIndex}-${workspaceIndex}-${paneIndex}`
                      return (
                        <button
                          aria-describedby={descriptionId}
                          aria-label={herdrPaneAccessibleName(
                            pane,
                            session.label,
                            pane.workspace_label ?? 'Ambiguous location',
                          )}
                          aria-pressed={selectedKey === pane.identity_key}
                          className="herdr-inventory__pane"
                          data-herdr-pane-row
                          data-kind={pane.kind}
                          data-observation={pane.observation}
                          data-stale={sessionStale}
                          key={pane.identity_key}
                          onClick={() => selectPane(pane)}
                          onFocus={() => {
                            focusedRowKey.current = pane.identity_key
                            activeIndex.current = Math.max(
                              0,
                              visiblePanes.findIndex(
                                ({ identity_key }) => identity_key === pane.identity_key,
                              ),
                            )
                            setActiveKey(pane.identity_key)
                          }}
                          onKeyDown={(event) => handleRowKey(event, pane)}
                          ref={(node) => {
                            if (node) rowRefs.current.set(pane.identity_key, node)
                            else rowRefs.current.delete(pane.identity_key)
                          }}
                          tabIndex={activeKey === pane.identity_key ? 0 : -1}
                          type="button"
                        >
                          <Icon aria-hidden="true" size={14} />
                          <span className="visually-hidden" id={descriptionId}>
                            {herdrPaneAccessibleDescription(pane, sessionStale)}
                          </span>
                          <span>
                            <strong>{label}</strong>
                            <small>
                              {pane.has_live_pane
                                ? (pane.display_provider ?? pane.provider ?? classification)
                                : classification}{' '}
                              · {status}
                            </small>
                          </span>
                          {pane.terminal_id ? <code>{pane.terminal_id}</code> : null}
                        </button>
                      )
                    })}
                  </div>
                ))}
              </section>
            )
          })}
          {!loading && !error && visiblePanes.length === 0 && !query ? (
            <p className="empty-state">No panes in running Herdr sessions.</p>
          ) : null}
          {!loading && query && visiblePanes.length === 0 ? (
            <p className="empty-state">No panes match this search.</p>
          ) : null}
          {loading && !inventory ? <p className="empty-state">Observing Herdr…</p> : null}
          {error && !inventory ? <p className="empty-state">No current Herdr snapshot.</p> : null}
        </nav>

        <article
          aria-label="Herdr pane details"
          className="herdr-inventory__details"
          onKeyDown={(event) => {
            if (event.key !== 'ArrowLeft' || !selected) return
            event.preventDefault()
            focusRow(selected.identity_key)
          }}
          ref={detailsRef}
          tabIndex={-1}
        >
          {selected ? (
            <>
              <button
                className="secondary-button herdr-inventory__return"
                onClick={() => focusRow(selected.identity_key)}
                ref={returnRef}
                type="button"
              >
                <ArrowLeft aria-hidden="true" size={14} />
                Back to pane list
              </button>
              <p className="eyebrow">
                {herdrPaneClassificationLabel(selected)}
              </p>
              <h2>{herdrPaneLabel(selected)}</h2>
              <p
                className="herdr-inventory__state"
                data-observation={selectedStale ? 'stale' : selected.observation}
              >
                {safeHerdrPaneReason(selected)}
              </p>
              {selectedStale ? (
                <p className="herdr-inventory__state" data-observation="stale">
                  Last observed details. Refresh for current information.
                </p>
              ) : null}
              {safeHerdrPaneMetadataReason(selected) ? (
                <p className="herdr-inventory__state" data-observation="ambiguous">
                  {safeHerdrPaneMetadataReason(selected)}
                </p>
              ) : null}
              <dl>
                <DetailRow label="Session" value={selectedSessionLabel} />
                <DetailRow label="Workspace" value={selected.workspace_label} />
                <DetailRow label="Tab" value={selected.tab_id} />
                <DetailRow label="Pane" value={selected.pane_id} />
                <DetailRow label="Terminal" value={selected.terminal_id} />
                <DetailRow label="Provider" value={selected.display_provider ?? selected.provider} />
                <DetailRow
                  label="Status"
                  value={selected.status === 'unknown' ? 'Status not reported' : selected.status}
                />
                <DetailRow label="Working directory" value={selected.foreground_cwd ?? selected.cwd} />
              </dl>
            </>
          ) : (
            <div className="empty-state">
              <strong>Select a pane</strong>
              <p>Details appear here.</p>
            </div>
          )}
        </article>
      </div>
    </section>
    </div>,
    document.body,
  )
}

import {
  lazy,
  Suspense,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react'
import { createPortal } from 'react-dom'
import {
  ArrowUpDown,
  Bot,
  Boxes,
  BriefcaseBusiness,
  Folder,
  GitBranch,
  Info,
  Map as MapIcon,
  MessageSquareText,
  Network,
  PanelLeftClose,
  RefreshCw,
  Search,
  Server,
  SquareTerminal,
  Files,
  X,
} from 'lucide-react'
import { AgentChatWorkspace } from './AgentChatWorkspace'
import {
  isTerminalWorkspaceMode,
  type AgentWorkspaceTarget,
  type AgentWorkspaceView,
  type TerminalPresentation,
} from './AgentWorkspaceContext'
import {
  filterAndSortAgentWindowGroups,
  groupAgentWindowTargets,
  selectAgentWindowSessionTargets,
  type AgentWindowSort,
} from './agentWindowNavigator'
import {
  runtimeCapabilityDetail,
  runtimeCapabilityLabel,
  type RuntimeCapabilities,
} from './runtimeCapabilities'
import { groupRuntimeLensEntries, runtimeLensEntryKey, runtimeLensEntryLabel } from './runtimeLensTargets'
import { useModalDialog } from './useModalDialog'
import type {
  CoordinationNodeRoute,
  Project,
  RuntimeInventory,
  RuntimeLensEntry,
  RuntimeSession,
  YardOrchestratorRoute,
} from './types'

const TerminalSession = lazy(() =>
  import('./TerminalSession').then((module) => ({
    default: module.TerminalSession,
  })),
)

function TargetIcon({
  target,
}: {
  target: AgentWorkspaceTarget
}) {
  if (target.role === 'superintendent') {
    return <Network aria-hidden="true" size={15} />
  }
  if (target.role === 'orchestrator' || target.role === 'workstream') {
    return <BriefcaseBusiness aria-hidden="true" size={15} />
  }
  return <Bot aria-hidden="true" size={15} />
}

const runtimeStatusOrder = [
  'blocked',
  'working',
  'idle',
  'done',
  'unknown',
] as const

const UNKNOWN_CONNECTION_DETAIL =
  'Worker not found in the latest Herdr snapshot. Yard cannot confirm that the bound terminal still exists.'

function targetCapabilities(target: AgentWorkspaceTarget): RuntimeCapabilities {
  return {
    chat: target.chatAvailable,
    reason: target.capabilityReason,
    terminal: target.interactive,
  }
}

function targetRuntimeSummary(targets: AgentWorkspaceTarget[]) {
  const interactiveTargets = targets.filter((target) => target.interactive)
  const unavailableCount = targets.length - interactiveTargets.length
  const statuses = runtimeStatusOrder
    .map((status) => ({
      count: interactiveTargets.filter(
        (target) => target.status === status,
      ).length,
      status,
    }))
    .filter(({ count }) => count > 0)
    .map(({ count, status }) => `${count} ${status}`)
  const parts = [`${targets.length} ${targets.length === 1 ? 'target' : 'targets'}`]
  if (statuses.length > 0) parts.push(`runtime ${statuses.join(', ')}`)
  if (unavailableCount > 0) {
    parts.push(`${unavailableCount} live controls unavailable`)
  }
  return parts.join(' · ')
}

function targetConnectionSummary(target: AgentWorkspaceTarget) {
  const capabilities = targetCapabilities(target)
  if (capabilities.reason === 'ready') {
    return `Observed runtime: ${target.status}`
  }
  const label = runtimeCapabilityLabel(capabilities)
  const detail =
    runtimeCapabilityDetail(capabilities) ?? UNKNOWN_CONNECTION_DETAIL
  return `${label}. ${detail}`
}

function targetTitle(target: AgentWorkspaceTarget) {
  const topology = [
    `terminal ${target.terminalId}`,
    target.tabId ? `tab ${target.tabId}` : null,
    `pane ${target.paneId}`,
  ]
    .filter(Boolean)
    .join(' · ')
  return [
    target.label,
    `${target.roleLabel} · ${target.contextLabel}`,
    `${target.harness} · ${target.runtimeAdapter} session ${target.session}`,
    targetConnectionSummary(target),
    topology,
    target.cwd,
  ]
    .filter(Boolean)
    .join('\n')
}

function targetDetailsControlLabel(target: AgentWorkspaceTarget) {
  return `Show runtime details for ${target.label}, ${target.contextLabel}, workspace ${target.workspaceId}, ${target.session} terminal ${target.terminalId}`
}

function AgentWindowDetailsDialog({
  onClose,
  returnFocus,
  target,
}: {
  onClose: () => void
  returnFocus: HTMLElement | null
  target: AgentWorkspaceTarget
}) {
  const dialogRef = useRef<HTMLElement>(null)
  useModalDialog({
    dialogRef,
    onClose,
    returnFocus,
  })

  const details = [
    ['Role', `${target.roleLabel} / ${target.contextLabel}`],
    ['Workspace', target.workspaceId],
    ['Herdr session', target.session],
    [
      'Runtime state',
      targetConnectionSummary(target),
    ],
    [
      'Live controls',
      !target.interactive
        ? 'Unavailable'
        : target.chatAvailable
          ? 'Chat and terminal available'
          : 'Terminal only',
    ],
    ['Terminal', target.terminalId],
    ['Tab', target.tabId ?? 'Not recorded'],
    ['Pane', target.paneId],
    ['Harness', `${target.harness} / ${target.runtimeAdapter}`],
    ['Working directory', target.cwd ?? 'Not reported'],
  ]

  return (
    <div
      className="modal-backdrop agent-window-details-backdrop"
      role="presentation"
    >
      <section
        aria-labelledby="agent-window-details-title"
        aria-modal="true"
        className="control-dialog agent-window-details-dialog"
        id="agent-window-details-dialog"
        onKeyDown={(event) => {
          if (event.key !== 'Escape') return
          event.preventDefault()
          event.stopPropagation()
          onClose()
        }}
        ref={dialogRef}
        role="dialog"
      >
        <header className="dialog-heading">
          <div>
            <p className="eyebrow">Runtime target</p>
            <h2 id="agent-window-details-title">{target.label}</h2>
          </div>
          <button
            aria-label="Close runtime details"
            className="icon-button"
            onClick={onClose}
            title="Close runtime details"
            type="button"
          >
            <X aria-hidden="true" size={17} />
          </button>
        </header>
        <dl className="agent-window-details-list">
          {details.map(([label, value]) => (
            <div key={label}>
              <dt>{label}</dt>
              <dd>{value}</dd>
            </div>
          ))}
        </dl>
      </section>
    </div>
  )
}

export function AgentWorkspaceShell({
  activeTarget,
  inventory,
  inventoryCurrent,
  lensEntries,
  mode,
  onCoordinationChange,
  onCoordinationNodeChange,
  onModeChange,
  onPresentationChange,
  onRefresh,
  onTargetChange,
  presentation,
  projects,
  returnFocus,
  selectedSession,
  sessions,
  targets,
  yardRoutes,
  coordinationRoutes,
}: {
  activeTarget: AgentWorkspaceTarget | null
  coordinationRoutes: CoordinationNodeRoute[]
  inventory: RuntimeInventory | null
  inventoryCurrent: boolean
  lensEntries: RuntimeLensEntry[]
  mode: AgentWorkspaceView
  onCoordinationChange: (route: YardOrchestratorRoute) => void
  onCoordinationNodeChange: (route: CoordinationNodeRoute) => void
  onModeChange: (mode: AgentWorkspaceView) => void
  onPresentationChange: (presentation: TerminalPresentation) => void
  onRefresh: () => void
  onTargetChange: (target: AgentWorkspaceTarget) => void
  presentation: TerminalPresentation
  projects: Project[]
  returnFocus: HTMLElement | null
  selectedSession: string
  sessions: RuntimeSession[]
  targets: AgentWorkspaceTarget[]
  yardRoutes: YardOrchestratorRoute[]
}) {
  const closeWorkspace = () => {
    onModeChange('map')
    window.setTimeout(
      () => (activeTarget?.returnFocus ?? returnFocus)?.focus(),
      0,
    )
  }

  const coordinationNode =
    activeTarget?.target.kind === 'coordination-node'
      ? activeTarget.target.node
      : null
  const chatProjects =
    coordinationNode
      ? projects.filter((project) =>
          coordinationNode.attached_project_ids.includes(
            project.id,
          ),
        )
      : projects
  const chatRoutes =
    activeTarget?.target.kind === 'yard-orchestrator'
      ? yardRoutes
      : coordinationNode
        ? coordinationRoutes.filter(
            (route) => route.node_id === coordinationNode.id,
          )
        : []
  const terminalVisible =
    isTerminalWorkspaceMode(mode) && Boolean(activeTarget?.interactive)
  const chatVisible =
    mode === 'chat' && Boolean(activeTarget?.chatAvailable)
  const renderedTarget = activeTarget ?? targets[0] ?? null
  const lastKnownRuntime = activeTarget
    ? [
        `session ${activeTarget.session}`,
        `terminal ${activeTarget.terminalId}`,
        activeTarget.paneId ? `pane ${activeTarget.paneId}` : null,
        activeTarget.cwd,
      ]
        .filter(Boolean)
        .join(' · ')
    : ''
  const navigatorRef = useRef<HTMLElement>(null)
  const mobileNavigatorTriggerRef = useRef<HTMLButtonElement>(null)
  const [detailsTarget, setDetailsTarget] = useState<{
    returnFocus: HTMLButtonElement
    target: AgentWorkspaceTarget
  } | null>(null)
  const [mobileNavigatorOpen, setMobileNavigatorOpen] = useState(false)
  const [windowQuery, setWindowQuery] = useState('')
  const [unassignedOpen, setUnassignedOpen] = useState(false)
  const [windowSort, setWindowSort] =
    useState<AgentWindowSort>('activity')
  useEffect(() => {
    if (mode !== 'chat') setMobileNavigatorOpen(false)
  }, [mode])
  useModalDialog({
    active:
      mode === 'chat' &&
      mobileNavigatorOpen &&
      detailsTarget === null,
    dialogRef: navigatorRef,
    onClose: () => setMobileNavigatorOpen(false),
    returnFocus: mobileNavigatorTriggerRef.current,
  })
  const sessionTargets = useMemo(
    () => selectAgentWindowSessionTargets(targets, selectedSession),
    [selectedSession, targets],
  )
  const pickerTargets = useMemo(
    () =>
      [...sessionTargets.selected].sort(
        (left, right) =>
          Number(!left.interactive) - Number(!right.interactive) ||
          Number(left.observation !== 'observed') -
            Number(right.observation !== 'observed') ||
          left.label.localeCompare(right.label),
      ),
    [sessionTargets.selected],
  )
  const groupedTargets = useMemo(() => {
    const selected = sessionTargets.selected
    const group = (candidates: AgentWorkspaceTarget[]) =>
      groupAgentWindowTargets(
        candidates,
        inventoryCurrent ? inventory : null,
        sessions,
      )
    return {
      pinned: group(
        selected.filter((target) => target.role === 'superintendent'),
      ),
      linked: group(
        selected.filter((target) => target.role !== 'superintendent'),
      ),
    }
  }, [inventory, inventoryCurrent, sessionTargets.selected, sessions])
  const visibleLinkedGroups = useMemo(
    () =>
      filterAndSortAgentWindowGroups(
        groupedTargets.linked,
        windowQuery,
        windowSort,
      ),
    [groupedTargets.linked, windowQuery, windowSort],
  )
  const unassignedGroups = useMemo(
    () =>
      groupRuntimeLensEntries(
        lensEntries,
        inventoryCurrent ? inventory : null,
        windowQuery,
        sessionTargets.selected,
      ),
    [
      inventory,
      inventoryCurrent,
      lensEntries,
      sessionTargets.selected,
      windowQuery,
    ],
  )
  const unassignedTargetCount = groupRuntimeLensEntries(
    lensEntries,
    inventoryCurrent ? inventory : null,
    '',
    sessionTargets.selected,
  ).reduce((count, group) => count + group.entries.length, 0)
  const unassignedExpanded =
    unassignedOpen ||
    (windowQuery.trim().length > 0 && unassignedGroups.length > 0)
  const controlledTargetCount = [
    ...groupedTargets.pinned,
    ...groupedTargets.linked,
  ].reduce((count, group) => count + group.targets.length, 0)
  const visibleWorkspaceGroups = [
    ...groupedTargets.pinned,
    ...visibleLinkedGroups,
  ]
  const visibleTargetCount = visibleWorkspaceGroups.reduce(
    (count, group) => count + group.targets.length,
    0,
  )
  const otherSessionCount = sessionTargets.elsewhereCount
  return (
    <section
      aria-label={activeTarget?.label ?? `${mode} workspace`}
      className="agent-workspace-shell"
      data-mobile-navigator-open={mobileNavigatorOpen || undefined}
      data-mode={mode}
      data-presentation={terminalVisible ? presentation : undefined}
      hidden={mode === 'map'}
      onKeyDown={(event) => {
        if (
          event.key !== 'Escape' ||
          mobileNavigatorOpen ||
          detailsTarget !== null
        ) {
          return
        }
        event.preventDefault()
        event.stopPropagation()
        closeWorkspace()
      }}
      role="region"
    >
      {mode !== 'map' ? (
        <aside
          aria-label="Herdr windows"
          aria-modal={mobileNavigatorOpen || undefined}
          className="agent-window-navigator"
          id="agent-window-navigator"
          onKeyDown={(event) => {
            if (event.key !== 'Escape' || !mobileNavigatorOpen) return
            event.preventDefault()
            event.stopPropagation()
            setMobileNavigatorOpen(false)
          }}
          ref={navigatorRef}
          role={mobileNavigatorOpen ? 'dialog' : undefined}
        >
        <header>
          <span className="agent-window-navigator__mark">
            <Server aria-hidden="true" size={16} />
          </span>
          <span>
            <strong>Herdr windows</strong>
            <small>
              {visibleTargetCount === controlledTargetCount
                ? `${controlledTargetCount} controlled`
                : `${visibleTargetCount} of ${controlledTargetCount}`}
              {` · ${selectedSession || 'no session'} selected`}
              {otherSessionCount > 0
                ? ` · ${otherSessionCount} linked elsewhere`
                : ''}
            </small>
          </span>
          <button
            aria-label="Close worker navigation"
            className="icon-button agent-window-navigator__close"
            onClick={() => setMobileNavigatorOpen(false)}
            title="Close worker navigation"
            type="button"
          >
            <X aria-hidden="true" size={16} />
          </button>
        </header>
        <div className="agent-window-navigator__controls">
          <label className="agent-window-search">
            <Search aria-hidden="true" size={12} />
            <input
              aria-label="Search Herdr windows"
              onChange={(event) => setWindowQuery(event.target.value)}
              placeholder="Find window"
              type="search"
              value={windowQuery}
            />
          </label>
          <label className="agent-window-sort">
            <ArrowUpDown aria-hidden="true" size={12} />
            <select
              aria-label="Sort Herdr windows"
              onChange={(event) =>
                setWindowSort(event.target.value as AgentWindowSort)
              }
              title="Sort Herdr windows"
              value={windowSort}
            >
              <option value="activity">Active</option>
              <option value="runtime">Herdr order</option>
              <option value="name">Name</option>
            </select>
          </label>
        </div>
        <div className="agent-window-navigator__workspaces">
          {unassignedTargetCount > 0 ? (
            <button
              aria-expanded={unassignedExpanded}
              className="agent-window-unassigned-toggle"
              onClick={() => setUnassignedOpen((current) => !current)}
              type="button"
            >
              <span>Other Herdr</span>
              <small>{unassignedTargetCount}</small>
            </button>
          ) : null}
          {unassignedExpanded
            ? unassignedGroups.map((group) => (
                <section
                  aria-label={`${group.label} other Herdr workspace`}
                  className="agent-window-workspace agent-window-workspace--unassigned"
                  data-workspace-id={group.workspaceId}
                  key={group.workspaceId}
                >
                  <header className="agent-window-lens-heading">
                    <strong>{group.label}</strong>
                    <small>
                      {group.workspaceId} · {group.entries.length}
                    </small>
                  </header>
                  {group.entries.map((entry) => (
                    <div
                      aria-label={`${runtimeLensEntryLabel(entry)}, ${entry.reason}`}
                      className="agent-window-lens-row"
                      data-classification={entry.classification}
                      key={runtimeLensEntryKey(entry)}
                    >
                      <strong>{runtimeLensEntryLabel(entry)}</strong>
                      <small>
                        Unbound · read-only unavailable
                      </small>
                      <code>
                        {entry.session} · {entry.terminal_id} ·{' '}
                        {entry.tab_id ? `tab ${entry.tab_id} · ` : ''}
                        pane {entry.pane_id}
                      </code>
                      <small>
                        {entry.reason} Yard must bind this pane before terminal
                        output or input is available.
                      </small>
                    </div>
                  ))}
                </section>
              ))
            : null}
          {visibleLinkedGroups.length === 0 && unassignedGroups.length === 0 ? (
            <p className="agent-window-navigator__empty" role="status">
              No matching windows
            </p>
          ) : null}
          {visibleWorkspaceGroups.map((group, groupIndex) => {
            const workspace = group.observation
            const label = workspace?.label ?? 'Unobserved workspace'
            const offline = group.sessionRunning === false
            const workspaceTitle = [
              `${label} · ${group.workspaceId}`,
              `${group.runtimeAdapter} session ${group.session}`,
              workspace
                ? `Observed workspace runtime: ${workspace.status}`
                : 'Durable target bindings; workspace not currently observed',
              workspace?.focused ? 'Focused workspace' : null,
              offline ? 'Session offline' : null,
              workspace
                ? `${workspace.tab_count} tabs · ${workspace.pane_count} panes`
                : null,
              workspace?.worktree
                ? `${workspace.worktree.repository_name} · ${workspace.worktree.checkout_path}`
                : null,
              targetRuntimeSummary(group.targets),
              'Runtime observations do not indicate workflow completion.',
            ]
              .filter(Boolean)
              .join('\n')
            return (
              <section
                aria-label={`${label} workspace ${group.workspaceId}`}
                className="agent-window-workspace"
                data-focused={workspace?.focused || undefined}
                data-observed={Boolean(workspace)}
                data-offline={offline || undefined}
                data-session={group.session}
                data-superintendent={group.targets.some(
                  (target) => target.role === 'superintendent',
                ) || undefined}
                data-workspace-id={group.workspaceId}
                key={`${group.targets.some((target) => target.role === 'superintendent') ? 'superintendent' : 'linked'}:${group.key}`}
              >
                <header
                  className="agent-window-workspace__heading"
                  title={workspaceTitle}
                >
                  <div className="agent-window-workspace__identity">
                    <Boxes aria-hidden="true" size={14} />
                    <span>
                      <strong>{label}</strong>
                      <code>{group.workspaceId}</code>
                    </span>
                    <small>
                      {group.targets.length}{' '}
                      {group.targets.length === 1 ? 'target' : 'targets'}
                    </small>
                  </div>
                  <div
                    className="agent-window-workspace__state"
                    data-status={offline ? 'offline' : workspace?.status}
                  >
                    <i aria-hidden="true" />
                    <span>
                      {offline
                        ? 'Session offline'
                        : workspace
                          ? `Observed ${workspace.status}`
                          : 'Durable bindings only'}
                    </span>
                    {workspace?.focused ? <b>Focused</b> : null}
                  </div>
                  <small className="agent-window-workspace__summary">
                    {targetRuntimeSummary(group.targets)}
                  </small>
                  <div className="agent-window-workspace__topology">
                    <span
                      title={`${group.runtimeAdapter} session ${group.session}`}
                    >
                      {group.session}
                    </span>
                    {workspace ? (
                      <span>
                        {workspace.tab_count} tabs · {workspace.pane_count}{' '}
                        panes
                      </span>
                    ) : (
                      <span>Topology unavailable</span>
                    )}
                  </div>
                  {workspace?.worktree ? (
                    <div className="agent-window-workspace__worktree">
                      <GitBranch aria-hidden="true" size={10} />
                      <span>{workspace.worktree.repository_name}</span>
                      <code>{workspace.worktree.checkout_path}</code>
                    </div>
                  ) : null}
                </header>
                {group.targets.map((target, targetIndex) => {
                  const descriptionId = `agent-window-target-${groupIndex}-${targetIndex}-description`
                  const detailsOpen = detailsTarget?.target.key === target.key
                  const detailsControlLabel =
                    targetDetailsControlLabel(target)
                  return (
                    <div
                      className="agent-window-row-frame"
                      key={target.key}
                    >
                      <button
                        aria-describedby={descriptionId}
                        aria-label={`${target.label}, ${target.roleLabel}, ${target.contextLabel}`}
                        aria-current={
                          target.key === activeTarget?.key
                            ? 'page'
                            : undefined
                        }
                        className="agent-window-row"
                        data-observation={target.observation}
                        data-status={target.status}
                        data-target-key={target.key}
                        onClick={() => {
                          onTargetChange(target)
                          setMobileNavigatorOpen(false)
                        }}
                        title={targetTitle(target)}
                        type="button"
                      >
                        <span
                          className="visually-hidden"
                          id={descriptionId}
                        >
                          {targetTitle(target)}
                        </span>
                        <span className="agent-window-row__icon">
                          <TargetIcon target={target} />
                          <i aria-hidden="true" />
                        </span>
                        <span className="agent-window-row__body">
                          <span className="agent-window-row__identity">
                            <strong>{target.label}</strong>
                            <small>
                              {!target.interactive
                                ? runtimeCapabilityLabel(
                                    targetCapabilities(target),
                                  )
                                : target.observation === 'stale'
                                ? 'Connection status stale'
                                : target.observation === 'observed'
                                  ? target.interactive
                                    ? target.chatAvailable
                                      ? `Runtime ${target.status}`
                                      : 'Observed · terminal only'
                                    : runtimeCapabilityLabel(
                                        targetCapabilities(target),
                                      )
                                  : 'Connection status unknown'}
                            </small>
                          </span>
                          <small className="agent-window-row__context">
                            {target.roleLabel} · {target.contextLabel}
                          </small>
                          <span className="agent-window-row__runtime">
                            <small>
                              {target.harness} · {target.session}
                            </small>
                            <code>{target.terminalId}</code>
                          </span>
                          <code className="agent-window-row__topology">
                            {target.tabId
                              ? `tab ${target.tabId} · `
                              : ''}
                            pane {target.paneId}
                          </code>
                          {target.cwd ? (
                            <span className="agent-window-row__path">
                              <Folder aria-hidden="true" size={9} />
                              <code>{target.cwd}</code>
                            </span>
                          ) : null}
                        </span>
                      </button>
                      <button
                        aria-controls="agent-window-details-dialog"
                        aria-expanded={detailsOpen}
                        aria-haspopup="dialog"
                        aria-label={detailsControlLabel}
                        className="icon-button agent-window-row__details"
                        onClick={(event) => {
                          const nextDetailsTarget = {
                            returnFocus:
                              mobileNavigatorOpen &&
                              mobileNavigatorTriggerRef.current
                                ? mobileNavigatorTriggerRef.current
                                : event.currentTarget,
                            target,
                          }
                          if (mobileNavigatorOpen) {
                            setMobileNavigatorOpen(false)
                            window.setTimeout(
                              () => setDetailsTarget(nextDetailsTarget),
                              0,
                            )
                            return
                          }
                          setDetailsTarget(nextDetailsTarget)
                        }}
                        title={detailsControlLabel}
                        type="button"
                      >
                        <Info aria-hidden="true" size={14} />
                      </button>
                    </div>
                  )
                })}
              </section>
            )
          })}
        </div>
        </aside>
      ) : null}

      {detailsTarget
        ? createPortal(
            <AgentWindowDetailsDialog
              onClose={() => setDetailsTarget(null)}
              returnFocus={detailsTarget.returnFocus}
              target={detailsTarget.target}
            />,
            document.body,
          )
        : null}

      {mode !== 'map' ? (
        <header className="agent-workspace-toolbar">
          <div
            aria-label="Agent workspace surface"
            className="workspace-mode-switcher agent-workspace-surfaces"
            onKeyDown={(event) => {
              if (
                !['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)
              ) {
                return
              }
              event.preventDefault()
              const tabs = Array.from(
                event.currentTarget.querySelectorAll<HTMLButtonElement>(
                  '[role="tab"]:not(:disabled)',
                ),
              )
              const current = tabs.indexOf(
                document.activeElement as HTMLButtonElement,
              )
              const next =
                event.key === 'Home'
                  ? 0
                  : event.key === 'End'
                    ? tabs.length - 1
                    : (current +
                        (event.key === 'ArrowRight' ? 1 : -1) +
                        tabs.length) %
                      tabs.length
              tabs[next]?.focus()
            }}
            role="tablist"
          >
            <button
              aria-selected={mode === 'chat'}
              disabled={Boolean(activeTarget && !activeTarget.chatAvailable)}
              onClick={() => onModeChange('chat')}
              role="tab"
              tabIndex={mode === 'chat' ? 0 : -1}
              type="button"
            >
              <MessageSquareText aria-hidden="true" size={14} />
              <span>Chat</span>
            </button>
            <button
              aria-selected={mode === 'terminal'}
              disabled={Boolean(activeTarget && !activeTarget.interactive)}
              onClick={() => onModeChange('terminal')}
              role="tab"
              tabIndex={mode === 'terminal' ? 0 : -1}
              type="button"
            >
              <SquareTerminal aria-hidden="true" size={14} />
              <span>Terminal</span>
            </button>
            <button
              aria-selected={mode === 'changes'}
              disabled={!activeTarget}
              onClick={() => onModeChange('changes')}
              role="tab"
              tabIndex={mode === 'changes' ? 0 : -1}
              type="button"
            >
              <Files aria-hidden="true" size={14} />
              <span>Files</span>
            </button>
          </div>
          {terminalVisible ? (
            <div
              aria-label="Terminal presentation"
              className="segmented-control terminal-presentation-control"
              role="group"
            >
              <button
                aria-label="Terminal"
                aria-pressed={presentation === 'terminal'}
                onClick={() => onPresentationChange('terminal')}
                title="Terminal presentation"
                type="button"
              >
                <SquareTerminal aria-hidden="true" size={14} />
                <span>Terminal</span>
              </button>
              <button
                aria-label="Focus"
                aria-pressed={presentation === 'focus'}
                onClick={() => onPresentationChange('focus')}
                title="Focus presentation"
                type="button"
              >
                <PanelLeftClose aria-hidden="true" size={14} />
                <span>Focus</span>
              </button>
            </div>
          ) : null}
          {mode === 'chat' ? (
            <button
              aria-controls="agent-window-navigator"
              aria-expanded={mobileNavigatorOpen}
              aria-label="Open worker navigation"
              className="icon-button agent-workspace-toolbar__mobile-navigator"
              onClick={() => setMobileNavigatorOpen(true)}
              ref={mobileNavigatorTriggerRef}
              title="Open worker navigation"
              type="button"
            >
              <Server aria-hidden="true" size={15} />
            </button>
          ) : null}
          <label className="agent-workspace-mobile-picker">
            <span className="visually-hidden">Agent target</span>
            <select
              aria-label={`Choose agent for ${mode}`}
              onChange={(event) => {
                const target = targets.find(
                  (candidate) => candidate.key === event.target.value,
                )
                if (target) onTargetChange(target)
              }}
              value={activeTarget?.key ?? ''}
            >
              <option value="">Choose agent</option>
              {pickerTargets.map((target) => (
                <option key={target.key} value={target.key}>
                  {target.label} ·{' '}
                  {target.interactive
                    ? `observed ${target.status}`
                    : runtimeCapabilityLabel(targetCapabilities(target))}
                </option>
              ))}
              {mode === 'terminal' && unassignedTargetCount > 0 ? (
                <optgroup label="Unbound Herdr panes">
                  {unassignedGroups.flatMap((group) =>
                    group.entries.map((entry) => (
                      <option
                        disabled
                        key={runtimeLensEntryKey(entry)}
                        value={`unbound:${runtimeLensEntryKey(entry)}`}
                      >
                        {runtimeLensEntryLabel(entry)} · Unbound · read-only
                        unavailable
                      </option>
                    )),
                  )}
                </optgroup>
              ) : null}
            </select>
          </label>
          <div className="agent-workspace-toolbar__target">
            <span data-status={activeTarget?.status} />
            <strong>{activeTarget?.label ?? 'Choose an agent'}</strong>
            <small>{activeTarget?.terminalId ?? selectedSession}</small>
            <button
              aria-label="Back to Map"
              className="icon-button"
              onClick={closeWorkspace}
              title="Back to Map"
              type="button"
            >
              <MapIcon aria-hidden="true" size={16} />
            </button>
          </div>
        </header>
      ) : null}

      <main className="agent-workspace-content">
        {renderedTarget ? (
          <AgentChatWorkspace
            closeOnEscape={
              !mobileNavigatorOpen && detailsTarget === null
            }
            label={renderedTarget.label}
            onCoordinationChange={onCoordinationChange}
            onCoordinationNodeChange={onCoordinationNodeChange}
            onClose={closeWorkspace}
            open={chatVisible}
            projects={chatProjects}
            returnFocus={null}
            routes={chatRoutes}
            status={renderedTarget.status}
            target={renderedTarget.target}
            variant="workspace"
          />
        ) : null}
        {!activeTarget ? (
          <div className="agent-workspace-picker-state" role="status">
            {mode === 'chat' ? (
              <MessageSquareText aria-hidden="true" size={22} />
            ) : (
              <SquareTerminal aria-hidden="true" size={22} />
            )}
            <strong>Choose an agent</strong>
            <span>
              {mode === 'chat'
                ? 'Select any Yard agent. Disconnected agents remain available with their last known status.'
                : 'Select a bound agent for terminal access. Unbound Herdr panes are listed without input control.'}
            </span>
          </div>
        ) : null}
        {activeTarget && mode === 'chat' && !activeTarget.chatAvailable ? (
          <div className="terminal-mode-loading" role="status">
            <strong>Agent chat unavailable</strong>
            <span>{targetConnectionSummary(activeTarget)}</span>
            <small>Last known runtime: {lastKnownRuntime}</small>
            <div className="inspector-actions">
              <button
                className="secondary-button"
                onClick={onRefresh}
                type="button"
              >
                <RefreshCw aria-hidden="true" size={15} />
                Refresh inventory
              </button>
            </div>
          </div>
        ) : null}
        {activeTarget && mode === 'terminal' && !activeTarget.interactive ? (
          <div className="terminal-mode-loading" role="status">
            <strong>Live controls unavailable</strong>
            <span>{targetConnectionSummary(activeTarget)}</span>
            <small>Last known runtime: {lastKnownRuntime}</small>
            <div className="inspector-actions">
              <button
                className="secondary-button"
                onClick={onRefresh}
                type="button"
              >
                <RefreshCw aria-hidden="true" size={15} />
                Refresh inventory
              </button>
            </div>
            <small>
              Reattach becomes available after Yard sees one current matching terminal.
            </small>
          </div>
        ) : null}
        {activeTarget && mode === 'changes' ? (
          <section
            aria-label={"Files for " + activeTarget.label}
            className="changes-workspace"
          >
            <header className="changes-workspace__header">
              <GitBranch aria-hidden="true" size={14} />
              <span>
                <strong>Repository files</strong>
                <small>{activeTarget.label}</small>
              </span>
            </header>
            <div className="changes-workspace__state" role="status">
              <strong>Repository browsing is not available yet</strong>
              <span>
                Yard no longer infers repository identity from terminal
                directories. Repository-backed browsing will return in the next
                slice.
              </span>
            </div>
          </section>
        ) : null}
        {terminalVisible && activeTarget ? (
          <Suspense
            fallback={
              <div className="terminal-mode-loading" role="status">
                Loading terminal
              </div>
            }
          >
            <TerminalSession
              key={activeTarget.terminalLeaseKey}
              target={activeTarget.terminalTarget}
            />
          </Suspense>
        ) : null}
      </main>
    </section>
  )
}

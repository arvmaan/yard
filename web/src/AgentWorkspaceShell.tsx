import {
  lazy,
  Suspense,
  useMemo,
  useRef,
  useState,
} from 'react'
import { createPortal } from 'react-dom'
import {
  Bot,
  Boxes,
  BriefcaseBusiness,
  Folder,
  GitBranch,
  Info,
  Network,
  PanelLeftClose,
  Server,
  SquareTerminal,
  X,
} from 'lucide-react'
import { AgentChatWorkspace } from './AgentChatWorkspace'
import {
  isTerminalWorkspaceMode,
  type AgentWorkspaceTarget,
  type AgentWorkspaceView,
  type TerminalPresentation,
} from './AgentWorkspaceContext'
import { groupAgentWindowTargets } from './agentWindowNavigator'
import { useModalDialog } from './useModalDialog'
import type {
  CoordinationNodeRoute,
  Project,
  RuntimeInventory,
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

function targetRuntimeSummary(targets: AgentWorkspaceTarget[]) {
  const observedTargets = targets.filter(
    (target) => target.observation === 'observed',
  )
  const durableCount = targets.length - observedTargets.length
  const statuses = runtimeStatusOrder
    .map((status) => ({
      count: observedTargets.filter(
        (target) => target.status === status,
      ).length,
      status,
    }))
    .filter(({ count }) => count > 0)
    .map(({ count, status }) => `${count} ${status}`)
  const parts = [`${targets.length} ${targets.length === 1 ? 'target' : 'targets'}`]
  if (statuses.length > 0) parts.push(`runtime ${statuses.join(', ')}`)
  if (durableCount > 0) parts.push(`${durableCount} durable-only`)
  return parts.join(' · ')
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
    target.observation === 'observed'
      ? `Observed runtime: ${target.status}`
      : 'Durable binding; runtime not currently observed',
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
      target.observation === 'observed'
        ? `Observed / ${target.status}`
        : 'Durable binding / not observed',
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
  mode,
  onCoordinationChange,
  onCoordinationNodeChange,
  onModeChange,
  onPresentationChange,
  onTargetChange,
  presentation,
  projects,
  sessions,
  targets,
  yardRoutes,
  coordinationRoutes,
}: {
  activeTarget: AgentWorkspaceTarget
  coordinationRoutes: CoordinationNodeRoute[]
  inventory: RuntimeInventory | null
  mode: AgentWorkspaceView
  onCoordinationChange: (route: YardOrchestratorRoute) => void
  onCoordinationNodeChange: (route: CoordinationNodeRoute) => void
  onModeChange: (mode: AgentWorkspaceView) => void
  onPresentationChange: (presentation: TerminalPresentation) => void
  onTargetChange: (target: AgentWorkspaceTarget) => void
  presentation: TerminalPresentation
  projects: Project[]
  sessions: RuntimeSession[]
  targets: AgentWorkspaceTarget[]
  yardRoutes: YardOrchestratorRoute[]
}) {
  const closeWorkspace = () => {
    onModeChange('map')
    window.setTimeout(() => activeTarget.returnFocus?.focus(), 0)
  }

  const coordinationNode =
    activeTarget.target.kind === 'coordination-node'
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
    activeTarget.target.kind === 'yard-orchestrator'
      ? yardRoutes
      : coordinationNode
        ? coordinationRoutes.filter(
            (route) => route.node_id === coordinationNode.id,
          )
        : []
  const terminalVisible = isTerminalWorkspaceMode(mode)
  const [detailsTarget, setDetailsTarget] = useState<{
    returnFocus: HTMLButtonElement
    target: AgentWorkspaceTarget
  } | null>(null)
  const workspaceGroups = useMemo(
    () => groupAgentWindowTargets(targets, inventory, sessions),
    [inventory, sessions, targets],
  )

  return (
    <section
      aria-label={activeTarget.label}
      className="agent-workspace-shell"
      data-presentation={terminalVisible ? presentation : undefined}
      hidden={mode === 'map'}
      role="dialog"
    >
      {mode !== 'map' ? (
        <aside
          aria-label="Herdr windows"
          className="agent-window-navigator"
        >
        <header>
          <span className="agent-window-navigator__mark">
            <Server aria-hidden="true" size={16} />
          </span>
          <span>
            <strong>Herdr windows</strong>
            <small>
              {targets.length} controlled · runtime evidence
            </small>
          </span>
        </header>
        <div className="agent-window-navigator__workspaces">
          {workspaceGroups.map((group, groupIndex) => {
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
                data-workspace-id={group.workspaceId}
                key={group.key}
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
                      <span>Topology not observed</span>
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
                          target.key === activeTarget.key
                            ? 'page'
                            : undefined
                        }
                        className="agent-window-row"
                        data-observation={target.observation}
                        data-status={target.status}
                        data-target-key={target.key}
                        onClick={() => onTargetChange(target)}
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
                              {target.observation === 'observed'
                                ? `Runtime ${target.status}`
                                : 'Not observed'}
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
                        onClick={(event) =>
                          setDetailsTarget({
                            returnFocus: event.currentTarget,
                            target,
                          })
                        }
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
          <div className="agent-workspace-toolbar__target">
            <span data-status={activeTarget.status} />
            <strong>{activeTarget.label}</strong>
            <small>{activeTarget.terminalId}</small>
            <button
              aria-label={mode === 'chat' ? 'Close chat' : 'Close terminal'}
              className="icon-button"
              onClick={closeWorkspace}
              title={mode === 'chat' ? 'Close chat' : 'Close terminal'}
              type="button"
            >
              <X aria-hidden="true" size={16} />
            </button>
          </div>
        </header>
      ) : null}

      <main className="agent-workspace-content">
        <AgentChatWorkspace
          label={activeTarget.label}
          onCoordinationChange={onCoordinationChange}
          onCoordinationNodeChange={onCoordinationNodeChange}
          onClose={() => onModeChange('map')}
          open={mode === 'chat'}
          projects={chatProjects}
          returnFocus={null}
          routes={chatRoutes}
          status={activeTarget.status}
          target={activeTarget.target}
          variant="workspace"
        />
        {terminalVisible ? (
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

import { lazy, Suspense } from 'react'
import {
  Bot,
  BriefcaseBusiness,
  Network,
  PanelLeftClose,
  Radio,
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
import type {
  CoordinationNodeRoute,
  Project,
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
  if (target.target.kind === 'yard-orchestrator') {
    return <Network aria-hidden="true" size={15} />
  }
  if (
    target.target.kind === 'orchestrator' ||
    target.target.kind === 'coordination-node'
  ) {
    return <BriefcaseBusiness aria-hidden="true" size={15} />
  }
  return <Bot aria-hidden="true" size={15} />
}

export function AgentWorkspaceShell({
  activeTarget,
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
            <small>{targets.length} under Yard control</small>
          </span>
        </header>
        <div className="agent-window-navigator__sessions">
          {sessions.map((session) => {
            const sessionTargets = targets.filter(
              (target) => target.session === session.name,
            )
            return (
              <section
                className="agent-window-session"
                data-running={session.running}
                key={session.name}
              >
                <div className="agent-window-session__heading">
                  <Radio aria-hidden="true" size={11} />
                  <strong>{session.name}</strong>
                  <small>
                    {session.running
                      ? sessionTargets.length
                      : 'offline'}
                  </small>
                </div>
                {sessionTargets.map((target) => (
                  <button
                    aria-current={
                      target.key === activeTarget.key
                        ? 'page'
                        : undefined
                    }
                    className="agent-window-row"
                    data-status={target.status}
                    key={target.key}
                    onClick={() => onTargetChange(target)}
                    title={`${target.label} · ${target.projectName}`}
                    type="button"
                  >
                    <span className="agent-window-row__icon">
                      <TargetIcon target={target} />
                      <i aria-hidden="true" />
                    </span>
                    <span>
                      <strong>{target.label}</strong>
                      <small>
                        {target.projectName} · {target.workspaceId}
                      </small>
                    </span>
                  </button>
                ))}
                {sessionTargets.length === 0 ? (
                  <p>No controlled windows</p>
                ) : null}
              </section>
            )
          })}
        </div>
        </aside>
      ) : null}

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

import {
  useEffect,
  useId,
  useRef,
  useState,
  type RefObject,
} from 'react'
import {
  Bot,
  Box,
  Boxes,
  ChevronDown,
  CircleAlert,
  FileCode2,
  FolderPlus,
  LoaderCircle,
  Map as MapIcon,
  MessageSquareText,
  RefreshCw,
  Settings,
  Settings2,
  SquareTerminal,
  Sun,
  Moon,
  Wifi,
  WifiOff,
  Workflow,
  X,
} from 'lucide-react'
import type { AgentWorkspaceMode } from './AgentWorkspaceContext'
import type { MapVisualMode } from './mapVisualMode'
import type { RuntimeSession } from './types'
import type { YardTheme } from './theme'
import { useModalDialog } from './useModalDialog'

export type ResourceView = 'profiles' | 'workspaces' | 'workers'

type RuntimeHealth = 'degraded' | 'loading' | 'observed' | 'unavailable'

interface RuntimeHealthPopoverProps {
  busy: boolean
  health: RuntimeHealth
  onRefresh: () => void
  onSessionChange: (session: string) => void
  selectedSession: string
  sessions: RuntimeSession[]
}

const HEALTH_LABELS: Record<RuntimeHealth, string> = {
  degraded: 'Observation delayed',
  loading: 'Observing Herdr',
  observed: 'Herdr observed',
  unavailable: 'Herdr unavailable',
}

function RuntimeHealthIcon({
  health,
  size,
}: {
  health: RuntimeHealth
  size: number
}) {
  if (health === 'loading') {
    return (
      <LoaderCircle
        aria-hidden="true"
        className="status-spin"
        size={size}
      />
    )
  }
  if (health === 'unavailable') {
    return <WifiOff aria-hidden="true" size={size} />
  }
  if (health === 'degraded') {
    return <CircleAlert aria-hidden="true" size={size} />
  }
  return <Wifi aria-hidden="true" size={size} />
}

export function RuntimeHealthPopover({
  busy,
  health,
  onRefresh,
  onSessionChange,
  selectedSession,
  sessions,
}: RuntimeHealthPopoverProps) {
  const popoverId = useId()
  const [open, setOpen] = useState(false)
  const containerRef = useRef<HTMLDivElement>(null)
  const selectRef = useRef<HTMLSelectElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const healthLabel = HEALTH_LABELS[health]
  const sessionLabel = selectedSession || 'No session'

  useEffect(() => {
    if (!open) return
    const frame = window.requestAnimationFrame(() => selectRef.current?.focus())
    const closeAndRestoreFocus = () => {
      setOpen(false)
      window.requestAnimationFrame(() => triggerRef.current?.focus())
    }
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      event.preventDefault()
      closeAndRestoreFocus()
    }
    const handlePointerDown = (event: PointerEvent) => {
      if (
        event.target instanceof Node &&
        !containerRef.current?.contains(event.target)
      ) {
        closeAndRestoreFocus()
      }
    }

    window.addEventListener('keydown', handleKeyDown)
    window.addEventListener('pointerdown', handlePointerDown)
    return () => {
      window.cancelAnimationFrame(frame)
      window.removeEventListener('keydown', handleKeyDown)
      window.removeEventListener('pointerdown', handlePointerDown)
    }
  }, [open])

  return (
    <div className="runtime-health" ref={containerRef}>
      <button
        aria-controls={popoverId}
        aria-expanded={open}
        aria-haspopup="dialog"
        aria-label={`Runtime health: ${sessionLabel}, ${healthLabel}`}
        className="runtime-health__trigger"
        data-health={health}
        onClick={() => setOpen((current) => !current)}
        ref={triggerRef}
        title={`Runtime health: ${sessionLabel}, ${healthLabel}`}
        type="button"
      >
        <RuntimeHealthIcon health={health} size={15} />
        <span className="runtime-health__session">{sessionLabel}</span>
        <span className="runtime-health__status">{healthLabel}</span>
        <ChevronDown aria-hidden="true" size={13} />
      </button>
      <span
        aria-live="polite"
        className="visually-hidden"
        role="status"
      >
        {sessionLabel}, {healthLabel}
      </span>
      {open ? (
        <section
          aria-label="Runtime health"
          className="runtime-health__popover"
          id={popoverId}
          role="dialog"
        >
          <header className="runtime-health__summary" data-health={health}>
            <RuntimeHealthIcon health={health} size={17} />
            <span>
              <strong>{healthLabel}</strong>
              <small>{sessionLabel}</small>
            </span>
          </header>
          <label className="runtime-health__field">
            <span>Herdr session</span>
            <select
              aria-label="Herdr session"
              disabled={sessions.length === 0}
              onChange={(event) => onSessionChange(event.target.value)}
              ref={selectRef}
              value={selectedSession}
            >
              {sessions.length === 0 ? (
                <option value="">No running sessions</option>
              ) : null}
              {sessions.map((session) => (
                <option
                  disabled={!session.running}
                  key={session.name}
                  value={session.name}
                >
                  {session.name}
                  {session.running ? '' : ' (stopped)'}
                </option>
              ))}
            </select>
          </label>
          <button
            className="runtime-health__refresh"
            disabled={busy}
            onClick={onRefresh}
            type="button"
          >
            <RefreshCw
              aria-hidden="true"
              className={busy ? 'status-spin' : ''}
              size={15}
            />
            <span>{busy ? 'Refreshing state' : 'Refresh state'}</span>
          </button>
        </section>
      ) : null}
    </div>
  )
}

interface GlobalCommandBarProps extends RuntimeHealthPopoverProps {
  activeAgentWorkspaceTarget: boolean
  agentWorkspaceMode: AgentWorkspaceMode | 'map'
  onAgentWorkspaceModeChange: (
    mode: AgentWorkspaceMode | 'map',
  ) => void
  onCreateProject: () => void
  onOpenSettings: () => void
  onResourceViewChange: (view: ResourceView) => void
  railView: ResourceView
  resourceShelfOpen: boolean
  settingsLabel: string
  settingsTriggerRef: RefObject<HTMLButtonElement | null>
}

const RESOURCE_ICONS = {
  profiles: FileCode2,
  workers: Bot,
  workspaces: Boxes,
}

export function GlobalCommandBar({
  activeAgentWorkspaceTarget,
  agentWorkspaceMode,
  busy,
  health,
  onAgentWorkspaceModeChange,
  onCreateProject,
  onOpenSettings,
  onRefresh,
  onResourceViewChange,
  onSessionChange,
  railView,
  resourceShelfOpen,
  selectedSession,
  sessions,
  settingsLabel,
  settingsTriggerRef,
}: GlobalCommandBarProps) {
  return (
    <header className="command-bar">
      <div aria-label="Yard" className="brand">
        <span className="brand__mark" aria-hidden="true">
          Y
        </span>
        <strong>Yard</strong>
      </div>

      <div
        aria-label="Application mode"
        className="workspace-mode-switcher command-bar__modes"
        role="tablist"
      >
        <button
          aria-selected={agentWorkspaceMode === 'map'}
          onClick={() => onAgentWorkspaceModeChange('map')}
          role="tab"
          title="Map"
          type="button"
        >
          <MapIcon aria-hidden="true" size={14} />
          <span>Map</span>
        </button>
        <button
          aria-selected={agentWorkspaceMode === 'chat'}
          disabled={!activeAgentWorkspaceTarget}
          onClick={() => onAgentWorkspaceModeChange('chat')}
          role="tab"
          title="Chat"
          type="button"
        >
          <MessageSquareText aria-hidden="true" size={14} />
          <span>Chat</span>
        </button>
        <button
          aria-selected={agentWorkspaceMode === 'terminal'}
          disabled={!activeAgentWorkspaceTarget}
          onClick={() => onAgentWorkspaceModeChange('terminal')}
          role="tab"
          title="Terminal"
          type="button"
        >
          <SquareTerminal aria-hidden="true" size={14} />
          <span>Terminal</span>
        </button>
      </div>

      <button
        aria-label="Create project"
        className="top-command"
        onClick={onCreateProject}
        title="Create project"
        type="button"
      >
        <FolderPlus aria-hidden="true" size={16} />
        <span>Create project</span>
      </button>

      <div
        aria-label="Observed resources"
        className="command-bar__resources"
        role="tablist"
      >
        {(['profiles', 'workers', 'workspaces'] as const).map((view) => {
          const Icon = RESOURCE_ICONS[view]
          const label =
            view === 'profiles'
              ? 'Profiles'
              : view === 'workers'
                ? 'Workers'
                : 'Workspaces'
          return (
            <button
              aria-controls="resource-shelf"
              aria-selected={resourceShelfOpen && railView === view}
              key={view}
              onClick={() => onResourceViewChange(view)}
              role="tab"
              title={label}
              type="button"
            >
              <Icon aria-hidden="true" size={13} />
              <span>{label}</span>
            </button>
          )
        })}
      </div>

      <RuntimeHealthPopover
        busy={busy}
        health={health}
        onRefresh={onRefresh}
        onSessionChange={onSessionChange}
        selectedSession={selectedSession}
        sessions={sessions}
      />

      <button
        aria-label={settingsLabel}
        className="icon-button settings-trigger"
        onClick={onOpenSettings}
        ref={settingsTriggerRef}
        title="Settings"
        type="button"
      >
        <Settings aria-hidden="true" size={17} />
      </button>
    </header>
  )
}

interface SettingsDialogProps {
  automaticCoordinationEnabledCount: number | null
  mapVisualMode: MapVisualMode
  onClose: () => void
  onOpenAutomaticCoordination: () => void
  onOpenOrchestratorWorkflow: () => void
  onMapVisualModeChange: (mode: MapVisualMode) => void
  onThemeChange: (theme: YardTheme) => void
  returnFocus: HTMLElement | null
  theme: YardTheme
  workflowProfileSummary: string | null
}

export function SettingsDialog({
  automaticCoordinationEnabledCount,
  mapVisualMode,
  onClose,
  onOpenAutomaticCoordination,
  onOpenOrchestratorWorkflow,
  onMapVisualModeChange,
  onThemeChange,
  returnFocus,
  theme,
  workflowProfileSummary,
}: SettingsDialogProps) {
  const titleId = useId()
  const dialogRef = useRef<HTMLElement>(null)
  const initialFocusRef = useRef<HTMLButtonElement>(null)
  const requestClose = useModalDialog({
    dialogRef,
    initialFocusRef,
    onClose,
    returnFocus,
  })

  return (
    <div className="modal-backdrop settings-backdrop" role="presentation">
      <section
        aria-labelledby={titleId}
        aria-modal="true"
        className="settings-dialog"
        ref={dialogRef}
        role="dialog"
      >
        <header className="settings-dialog__header">
          <div>
            <p className="eyebrow">Preferences</p>
            <h2 id={titleId}>Settings</h2>
          </div>
          <button
            aria-label="Close settings"
            className="icon-button"
            onClick={requestClose}
            title="Close settings"
            type="button"
          >
            <X aria-hidden="true" size={17} />
          </button>
        </header>
        <div className="settings-dialog__body">
          <section aria-labelledby={`${titleId}-appearance`}>
            <h3 id={`${titleId}-appearance`}>Appearance</h3>
            <div className="settings-row">
              <div>
                <strong>Theme</strong>
                <small>Stored in this browser</small>
              </div>
              <div
                aria-label="Theme"
                className="segmented-control settings-segmented-control"
                role="group"
              >
                <button
                  aria-pressed={theme === 'light'}
                  onClick={() => onThemeChange('light')}
                  ref={initialFocusRef}
                  type="button"
                >
                  <Sun aria-hidden="true" size={14} />
                  Light
                </button>
                <button
                  aria-pressed={theme === 'dark'}
                  onClick={() => onThemeChange('dark')}
                  type="button"
                >
                  <Moon aria-hidden="true" size={14} />
                  Dark
                </button>
              </div>
            </div>
            <div className="settings-row">
              <div>
                <strong>Map view</strong>
                <small>2D remains available as the fallback</small>
              </div>
              <div
                aria-label="Map view"
                className="segmented-control settings-segmented-control"
                role="group"
              >
                <button
                  aria-label="2D view"
                  aria-pressed={mapVisualMode === 'flat'}
                  onClick={() => onMapVisualModeChange('flat')}
                  type="button"
                >
                  <MapIcon aria-hidden="true" size={14} />
                  2D
                </button>
                <button
                  aria-label="2.5D view"
                  aria-pressed={mapVisualMode === 'depth'}
                  onClick={() => onMapVisualModeChange('depth')}
                  type="button"
                >
                  <Box aria-hidden="true" size={14} />
                  2.5D
                </button>
              </div>
            </div>
          </section>
          <section aria-labelledby={`${titleId}-coordination`}>
            <h3 id={`${titleId}-coordination`}>Coordination</h3>
            <div className="settings-row">
              <div>
                <strong>Automatic coordination</strong>
                <small>
                  {automaticCoordinationEnabledCount === null
                    ? 'Durable settings unavailable'
                    : automaticCoordinationEnabledCount === 0
                      ? 'Off by default; automatic requests may spend tokens'
                      : `${automaticCoordinationEnabledCount} of 3 automatic behaviors enabled`}
                </small>
              </div>
              <button
                aria-label="Automatic token use settings"
                className="secondary-button settings-action"
                disabled={automaticCoordinationEnabledCount === null}
                onClick={onOpenAutomaticCoordination}
                type="button"
              >
                <Settings2 aria-hidden="true" size={14} />
                Configure
              </button>
            </div>
            <div className="settings-row">
              <div>
                <strong>Orchestrator workflow</strong>
                <small>
                  {workflowProfileSummary ?? 'Durable profile unavailable'}
                </small>
              </div>
              <button
                aria-label="Orchestrator workflow settings"
                className="secondary-button settings-action"
                disabled={workflowProfileSummary === null}
                onClick={onOpenOrchestratorWorkflow}
                type="button"
              >
                <Workflow aria-hidden="true" size={14} />
                Edit
              </button>
            </div>
          </section>
        </div>
      </section>
    </div>
  )
}

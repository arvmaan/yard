import {
  useEffect,
  useId,
  useRef,
  useState,
  type RefObject,
} from 'react'
import {
  Activity,
  Bot,
  Box,
  Boxes,
  ChevronDown,
  CircleAlert,
  FileCode2,
  FolderPlus,
  LoaderCircle,
  Map as MapIcon,
  RefreshCw,
  Settings,
  Wifi,
  WifiOff,
  Workflow,
  X,
  Zap,
} from 'lucide-react'
import type { MapVisualMode } from './mapVisualMode'
import type {
  RuntimeSession,
  TokenSpendSettings,
} from './types'
import { THEME_OPTIONS, type ThemeId } from './theme'
import { useModalDialog } from './useModalDialog'

export type ResourceView = 'profiles' | 'workspaces' | 'workers'

type RuntimeHealth = 'degraded' | 'loading' | 'observed' | 'unavailable'

interface RuntimeHealthPopoverProps {
  busy: boolean
  health: RuntimeHealth
  herdrInventoryOpen: boolean
  herdrInventoryTriggerRef: RefObject<HTMLButtonElement | null>
  onOpenHerdrInventory: () => void
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
  herdrInventoryOpen,
  herdrInventoryTriggerRef,
  onOpenHerdrInventory,
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
    return () => window.cancelAnimationFrame(frame)
  }, [open])

  useEffect(() => {
    if (!open || herdrInventoryOpen) return
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
      window.removeEventListener('keydown', handleKeyDown)
      window.removeEventListener('pointerdown', handlePointerDown)
    }
  }, [herdrInventoryOpen, open])

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
          <button
            aria-controls="herdr-inventory-workspace"
            aria-expanded={herdrInventoryOpen}
            className="runtime-health__refresh"
            onClick={onOpenHerdrInventory}
            ref={herdrInventoryTriggerRef}
            type="button"
          >
            <Boxes aria-hidden="true" size={15} />
            <span>Open Herdr inventory</span>
          </button>
        </section>
      ) : null}
    </div>
  )
}

interface GlobalCommandBarProps extends RuntimeHealthPopoverProps {
  attentionCount: number
  automaticCoordinationLabel: string
  automaticCoordinationEnabledCount: number
  onCreateProject: () => void
  onHome: () => void
  onOpenAttention: (trigger: HTMLButtonElement) => void
  onOpenAutomaticCoordination: () => void
  onOpenProjectPulse: (trigger: HTMLButtonElement) => void
  onOpenSettings: () => void
  onToggleResources: (trigger: HTMLButtonElement) => void
  projectPulseTriggerRef: RefObject<HTMLButtonElement | null>
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
  attentionCount,
  automaticCoordinationEnabledCount,
  automaticCoordinationLabel,
  busy,
  health,
  herdrInventoryOpen,
  herdrInventoryTriggerRef,
  onCreateProject,
  onHome,
  onOpenAttention,
  onOpenAutomaticCoordination,
  onOpenHerdrInventory,
  onOpenProjectPulse,
  onOpenSettings,
  onRefresh,
  onSessionChange,
  onToggleResources,
  projectPulseTriggerRef,
  resourceShelfOpen,
  selectedSession,
  sessions,
  settingsLabel,
  settingsTriggerRef,
}: GlobalCommandBarProps) {
  return (
    <header className="command-bar">
      <button
        aria-label="Yard map"
        className="brand"
        onClick={onHome}
        title="Return to map"
        type="button"
      >
        <span className="brand__mark" aria-hidden="true">
          Y
        </span>
        <strong>Yard</strong>
      </button>

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

      <button
        aria-controls="resource-shelf"
        aria-expanded={resourceShelfOpen}
        className="top-command top-command--secondary"
        onClick={(event) => onToggleResources(event.currentTarget)}
        title="Resources"
        type="button"
      >
        <Boxes aria-hidden="true" size={16} />
        <span>Resources</span>
      </button>

      {attentionCount > 0 ? (
        <button
          aria-label={`${attentionCount} workers need attention`}
          className="chrome-telltale chrome-telltale--attention"
          onClick={(event) => onOpenAttention(event.currentTarget)}
          title={`${attentionCount} workers need attention`}
          type="button"
        >
          <CircleAlert aria-hidden="true" size={15} />
          <span>{attentionCount}</span>
        </button>
      ) : null}

      <button
        aria-label="Project pulse"
        className="top-command top-command--secondary project-pulse-trigger"
        onClick={(event) => onOpenProjectPulse(event.currentTarget)}
        ref={projectPulseTriggerRef}
        title="Project pulse"
        type="button"
      >
        <Activity aria-hidden="true" size={16} />
        <span>Project pulse</span>
      </button>

      <RuntimeHealthPopover
        busy={busy}
        health={health}
        herdrInventoryOpen={herdrInventoryOpen}
        herdrInventoryTriggerRef={herdrInventoryTriggerRef}
        onOpenHerdrInventory={onOpenHerdrInventory}
        onRefresh={onRefresh}
        onSessionChange={onSessionChange}
        selectedSession={selectedSession}
        sessions={sessions}
      />

      {automaticCoordinationEnabledCount > 0 ? (
        <button
          aria-label={automaticCoordinationLabel}
          className="chrome-telltale chrome-telltale--automatic"
          onClick={onOpenAutomaticCoordination}
          title={automaticCoordinationLabel}
          type="button"
        >
          <Zap aria-hidden="true" size={14} />
          <span>Auto {automaticCoordinationEnabledCount}</span>
        </button>
      ) : null}

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

export function ResourceShelfTabs({
  counts,
  onChange,
  value,
}: {
  counts: Record<ResourceView, number>
  onChange: (view: ResourceView) => void
  value: ResourceView
}) {
  return (
    <nav
      aria-label="Resources"
      className="resource-shelf__tabs"
      onKeyDown={(event) => {
        if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) {
          return
        }
        event.preventDefault()
        const tabs = Array.from(
          event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="tab"]'),
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
            aria-selected={value === view}
            key={view}
            onClick={() => onChange(view)}
            role="tab"
            tabIndex={value === view ? 0 : -1}
            type="button"
          >
            <Icon aria-hidden="true" size={13} />
            <span>{label}</span>
            <small>{counts[view]}</small>
          </button>
        )
      })}
    </nav>
  )
}

export interface AutomaticCoordinationSelection {
  projectOrchestrators: boolean
  scheduled: boolean
  superintendent: boolean
}

interface SettingsDialogProps {
  automaticCoordinationBusy: boolean
  automaticCoordinationError: string | null
  automaticCoordinationSettings: TokenSpendSettings | null
  initialFocus: 'appearance' | 'automatic'
  mapVisualMode: MapVisualMode
  onClose: () => void
  onSaveAutomaticCoordination: (
    selection: AutomaticCoordinationSelection,
  ) => void
  onOpenOrchestratorWorkflow: () => void
  onMapVisualModeChange: (mode: MapVisualMode) => void
  onThemeChange: (theme: ThemeId) => void
  returnFocus: HTMLElement | null
  theme: ThemeId
  workflowProfileSummary: string | null
}

export function SettingsDialog({
  automaticCoordinationBusy,
  automaticCoordinationError,
  automaticCoordinationSettings,
  initialFocus,
  mapVisualMode,
  onClose,
  onSaveAutomaticCoordination,
  onOpenOrchestratorWorkflow,
  onMapVisualModeChange,
  onThemeChange,
  returnFocus,
  theme,
  workflowProfileSummary,
}: SettingsDialogProps) {
  const titleId = useId()
  const dialogRef = useRef<HTMLElement>(null)
  const appearanceFocusRef = useRef<HTMLSelectElement>(null)
  const automaticFocusRef = useRef<HTMLInputElement>(null)
  const [superintendent, setSuperintendent] = useState(
    automaticCoordinationSettings?.superintendent_auto_requests_project_summaries ??
      false,
  )
  const [projectOrchestrators, setProjectOrchestrators] = useState(
    automaticCoordinationSettings?.project_orchestrators_auto_request_worker_summaries ??
      false,
  )
  const [scheduled, setScheduled] = useState(
    automaticCoordinationSettings?.scheduled_automatic_summaries ?? false,
  )
  useEffect(() => {
    if (!automaticCoordinationSettings) return
    setSuperintendent(
      automaticCoordinationSettings.superintendent_auto_requests_project_summaries,
    )
    setProjectOrchestrators(
      automaticCoordinationSettings.project_orchestrators_auto_request_worker_summaries,
    )
    setScheduled(automaticCoordinationSettings.scheduled_automatic_summaries)
  }, [automaticCoordinationSettings])
  const automaticDirty =
    automaticCoordinationSettings !== null &&
    (superintendent !==
      automaticCoordinationSettings.superintendent_auto_requests_project_summaries ||
      projectOrchestrators !==
        automaticCoordinationSettings.project_orchestrators_auto_request_worker_summaries ||
      scheduled !==
        automaticCoordinationSettings.scheduled_automatic_summaries)
  const requestClose = useModalDialog({
    canClose: !automaticCoordinationBusy,
    dialogRef,
    initialFocusRef:
      initialFocus === 'automatic' ? automaticFocusRef : appearanceFocusRef,
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
              <select
                aria-label="Theme"
                className="settings-theme-select"
                onChange={(event) => onThemeChange(event.target.value)}
                ref={appearanceFocusRef}
                value={theme}
              >
                {THEME_OPTIONS.map((option) => (
                  <option key={option.id} value={option.id}>
                    {option.label}
                  </option>
                ))}
              </select>
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
          <section aria-labelledby={`${titleId}-automatic`}>
            <h3 id={`${titleId}-automatic`}>Automatic coordination</h3>
            <p className="settings-section-copy">
              Automatic requests may use provider tokens. Manual requests and
              Run now remain available when these are off.
            </p>
            <label className="settings-switch-row">
              <span>
                <strong>Request project summaries automatically</strong>
                <small>Superintendent to project orchestrators</small>
              </span>
              <input
                checked={superintendent}
                disabled={!automaticCoordinationSettings}
                onChange={(event) => setSuperintendent(event.target.checked)}
                ref={automaticFocusRef}
                role="switch"
                type="checkbox"
              />
            </label>
            <label className="settings-switch-row">
              <span>
                <strong>Request worker summaries automatically</strong>
                <small>Project orchestrators to workers</small>
              </span>
              <input
                checked={projectOrchestrators}
                disabled={!automaticCoordinationSettings}
                onChange={(event) =>
                  setProjectOrchestrators(event.target.checked)
                }
                role="switch"
                type="checkbox"
              />
            </label>
            <label className="settings-switch-row">
              <span>
                <strong>Run scheduled summaries automatically</strong>
                <small>Unrelated automations and Run now stay available</small>
              </span>
              <input
                checked={scheduled}
                disabled={!automaticCoordinationSettings}
                onChange={(event) => setScheduled(event.target.checked)}
                role="switch"
                type="checkbox"
              />
            </label>
            <div className="settings-save-row">
              <small>
                {automaticCoordinationSettings
                  ? automaticDirty
                    ? 'Unsaved automatic coordination changes'
                    : 'Durable settings are current'
                  : 'Durable settings unavailable'}
              </small>
              <button
                className="command-button"
                disabled={
                  automaticCoordinationBusy ||
                  !automaticCoordinationSettings ||
                  !automaticDirty
                }
                onClick={() =>
                  onSaveAutomaticCoordination({
                    projectOrchestrators,
                    scheduled,
                    superintendent,
                  })
                }
                type="button"
              >
                {automaticCoordinationBusy ? 'Saving' : 'Save automatic settings'}
              </button>
            </div>
            {automaticCoordinationError ? (
              <p className="dialog-error" role="alert">
                <CircleAlert aria-hidden="true" size={15} />
                <span>{automaticCoordinationError}</span>
              </p>
            ) : null}
          </section>
          <section aria-labelledby={`${titleId}-coordination`}>
            <h3 id={`${titleId}-coordination`}>Coordination</h3>
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

import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type FormEvent,
} from 'react'
import {
  ArrowRightLeft,
  Bot,
  Boxes,
  BriefcaseBusiness,
  CircleAlert,
  CircleCheck,
  CircleHelp,
  CirclePlay,
  CircleStop,
  ChevronUp,
  FileCode2,
  FolderArchive,
  FolderPlus,
  GitBranch,
  GripVertical,
  LoaderCircle,
  Network,
  Pause,
  Pencil,
  Plus,
  RefreshCw,
  Server,
  Trash2,
  Unlink,
  Wifi,
  WifiOff,
  X,
} from 'lucide-react'
import {
  YardApiError,
  archiveProject,
  changeProjectOrchestrator,
  confirmWorkerHandoff,
  confirmWorkerAssignment,
  createAutomation,
  createCoordinationNode,
  createProject,
  createProjectFromProfile,
  createProjectRelationship,
  createProjectWithWorkspace,
  createWorkerProfile,
  deleteProject,
  deleteProjectRelationship,
  deleteWorker,
  endWorkerSession,
  fetchAutomation,
  fetchAutomationRuns,
  fetchAutomations,
  fetchInventory,
  fetchRuntimeTopology,
  fetchCoordinationNodeRoutes,
  fetchCoordinationNodes,
  fetchCoordinationSnapshots,
  fetchOrchestratorWorkflowProfile,
  fetchOrchestratorStatusOutput,
  fetchProject,
  fetchProjectAssignments,
  fetchProjectRelationships,
  fetchProjects,
  fetchSessions,
  fetchTokenSpendSettings,
  fetchYardOrchestrator,
  fetchYardOrchestratorRoutes,
  fetchWorkerProfiles,
  fetchWorkers,
  provisionYardOrchestrator,
  recoverYardOrchestrator,
  resetOrchestratorWorkflowProfile,
  provisionCoordinationNode,
  recordCompletionReceipt,
  requestCoordinationSnapshot,
  runAutomation,
  uploadArtifact,
  updateAutomation,
  updateAutomationPlacement,
  updateAutomationState,
  updateProjectPlacement,
  updateTokenSpendSettings,
  updateCoordinationNode,
  updateCoordinationNodePlacement,
  updateOrchestratorWorkflowProfile,
  updateWorkerProfile,
} from './api'
import {
  RuntimeCanvas,
  type CanvasSelection,
} from './RuntimeCanvas'
import {
  GlobalCommandBar,
  SettingsDialog,
} from './AppChrome'
import {
  AgentGroupChat,
  type AgentGroupTarget,
} from './AgentGroupChat'
import {
  setAllocationDragData,
  type AllocationDragPayload,
} from './allocationDrag'
import {
  AllocationDialog,
  type AllocationDetails,
  type AllocationSubject,
} from './AllocationDialog'
import { WorkerInterventions } from './WorkerInterventions'
import {
  AgentWorkspaceContext,
  DEFAULT_TERMINAL_PRESENTATION,
  terminalLeaseKey,
  type AgentWorkspaceTarget,
  type AgentWorkspaceView,
  type TerminalPresentation,
} from './AgentWorkspaceContext'
import { AgentWorkspaceShell } from './AgentWorkspaceShell'
import { ProjectPulseWorkspace } from './ProjectPulseWorkspace'
import {
  resolveRuntimeCapabilities,
  runtimeCapabilityDetail,
  runtimeCapabilityLabel,
  runtimeCapabilityObservationState,
  runtimeCapabilityProcessState,
  runtimeCapabilityStatus,
  type ResolvedRuntimeCapabilities,
  type RuntimeCapabilities,
} from './runtimeCapabilities'
import {
  CoordinationNodeDialog,
  type CoordinationNodeCreationDetails,
} from './CoordinationNodeDialog'
import { CoordinationNodeInspector } from './CoordinationNodeInspector'
import {
  AutomationDialog,
  type AutomationDetails,
} from './AutomationDialog'
import { AutomationInspector } from './AutomationInspector'
import { TokenSpendSettingsDialog } from './TokenSpendSettingsDialog'
import { OrchestratorWorkflowProfileDialog } from './OrchestratorWorkflowProfileDialog'
import {
  parseStatusReport,
  statusReportMatchesLatestRoute,
  type ProjectStatusReports,
} from './projectUpdates'
import { EndWorkerSessionDialog } from './EndWorkerSessionDialog'
import { ArchiveProjectDialog } from './ArchiveProjectDialog'
import { DeleteProjectDialog } from './DeleteProjectDialog'
import { DeleteWorkerDialog } from './DeleteWorkerDialog'
import {
  HandoffDialog,
  type HandoffDetails,
} from './HandoffDialog'
import {
  CompletionDialog,
  type CompletionDetails,
} from './CompletionDialog'
import { ProfileEditor } from './ProfileEditor'
import { nextProjectPlacement } from './projectLayout'
import {
  defaultProjectAccent,
  PROJECT_ACCENTS,
  readProjectAccents,
  writeProjectAccents,
} from './projectAppearance'
import {
  applyTheme,
  readTheme,
  themeDefinition,
  type ThemeId,
} from './theme'
import {
  reconcileInventorySnapshot,
  reconcileRuntimeProjectionSnapshot,
} from './inventoryState'
import {
  projectOrchestratorEligibility,
  type ProjectOrchestratorEligibilityReason,
} from './projectOrchestratorEligibility'
import {
  beginProjectTransferRefresh,
  emptyProjectTransferContext,
  failProjectTransferRefresh,
  resolveProjectTransferRefresh,
  type ProjectTransferContextEntry,
  type ProjectTransferSnapshot,
} from './projectTransferContext'
import {
  readMapVisualMode,
  writeMapVisualMode,
  type MapVisualMode,
} from './mapVisualMode'
import { useModalDialog } from './useModalDialog'
import type {
  Artifact,
  Assignment,
  Automation,
  AutomationRun,
  AutomationScope,
  CanvasPlacement,
  CoordinationNode,
  CoordinationNodeKind,
  CoordinationNodeRoute,
  CoordinationSnapshot,
  CreateWorkerProfileInput,
  ObservedChildAgent,
  ObservedStatus,
  ObservedWorker,
  OrchestratorWorkflowProfile,
  Project,
  ProjectRelationship,
  RuntimeInventory,
  RuntimeObservationState,
  RuntimeProcessState,
  RuntimeSession,
  RuntimeTopology,
  StatusReport,
  TokenSpendSettings,
  WorkerAvailability,
  WorkerCandidate,
  WorkerProfile,
  WorkerRuntimeBinding,
  WorkflowStatus,
  WorkspaceObservation,
  YardOrchestrator,
  YardOrchestratorRoute,
} from './types'
import './App.css'

const INVENTORY_REFRESH_INTERVAL_MS = 1_000
const ASSIGNMENT_REFRESH_INTERVAL_MS = 5_000
const PROJECT_STATUS_REFRESH_INTERVAL_MS = 15_000

const ArtifactInspector = lazy(() =>
  import('./ArtifactInspector').then((module) => ({
    default: module.ArtifactInspector,
  })),
)

type Filter = 'current' | 'available' | 'allocated' | 'attention' | 'history'
type RailView = 'profiles' | 'workspaces' | 'workers'
type ProjectCreationDetails =
  | {
      mode: 'existing'
      name: string
      orchestratorId: string
    }
  | {
      commandId: string
      mode: 'profile'
      name: string
      objective: string
      profileId: string
    }

interface WorkspaceProjectCreationDetails {
  commandId: string
  cwd: string
  name: string
  objective: string
  profileId: string
}

const FILTERS: Array<{ value: Filter; label: string }> = [
  { value: 'current', label: 'Current' },
  { value: 'available', label: 'Available' },
  { value: 'allocated', label: 'Allocated' },
  { value: 'attention', label: 'Attention' },
  { value: 'history', label: 'History' },
]

const STATUS_ICONS = {
  blocked: CircleAlert,
  done: CircleCheck,
  idle: Pause,
  unknown: CircleHelp,
  working: LoaderCircle,
} satisfies Record<ObservedStatus, typeof CircleAlert>

const AVAILABILITY_ICONS = {
  ambiguous: CircleHelp,
  assigned: Bot,
  ended: CircleStop,
  orchestrator: BriefcaseBusiness,
  resumable: RefreshCw,
  unavailable: WifiOff,
  unassigned_live: CircleCheck,
  yard_orchestrator: Network,
} satisfies Record<WorkerAvailability, typeof CircleAlert>

const AVAILABILITY_LABELS = {
  ambiguous: 'Ambiguous',
  assigned: 'Assigned',
  ended: 'Ended',
  orchestrator: 'Orchestrator',
  resumable: 'Replacement ready',
  unavailable: 'Unavailable',
  unassigned_live: 'Unassigned live',
  yard_orchestrator: 'Superintendent',
} satisfies Record<WorkerAvailability, string>

const PROCESS_ICONS = {
  exited: CircleStop,
  running: CirclePlay,
  unknown: CircleHelp,
} satisfies Record<RuntimeProcessState, typeof CircleAlert>

const OBSERVATION_ICONS = {
  ambiguous: CircleHelp,
  missing: WifiOff,
  observed: Wifi,
} satisfies Record<RuntimeObservationState, typeof CircleAlert>

function matchesFilter(candidate: WorkerCandidate, filter: Filter) {
  if (filter === 'current') return candidate.availability !== 'ended'
  if (filter === 'history') return candidate.availability === 'ended'
  if (filter === 'available') {
    return (
      candidate.availability === 'unassigned_live' ||
      candidate.availability === 'resumable'
    )
  }
  if (filter === 'allocated') {
    return (
      candidate.availability === 'orchestrator' ||
      candidate.availability === 'yard_orchestrator' ||
      candidate.availability === 'assigned'
    )
  }
  if (filter === 'attention') {
    return (
      candidate.availability === 'unavailable' ||
      candidate.availability === 'ambiguous' ||
      candidate.worker.runtime?.status === 'blocked' ||
      candidate.worker.runtime?.status === 'done' ||
      candidate.worker.runtime?.status === 'unknown'
    )
  }
  return false
}

function workerLabel(worker: ObservedWorker) {
  return worker.name ?? worker.display_provider ?? worker.provider ?? 'Worker'
}

function candidateLabel(candidate: WorkerCandidate) {
  return (
    candidate.profile_name ??
    `Worker ${candidate.worker.id.slice(0, 8)}`
  )
}

function canAllocateCandidate(candidate: WorkerCandidate) {
  return (
    candidate.availability === 'unassigned_live' ||
    candidate.availability === 'resumable'
  )
}

function canHandoffCandidate(candidate: WorkerCandidate) {
  return (
    candidate.availability === 'assigned' &&
    Boolean(candidate.project_id) &&
    Boolean(candidate.assignment_id)
  )
}

function canEndCandidate(candidate: WorkerCandidate) {
  return (
    candidate.worker.desired_state === 'running' &&
    candidate.availability !== 'yard_orchestrator' &&
    candidate.availability !== 'orchestrator' &&
    candidate.availability !== 'assigned'
  )
}

function canDeleteCandidate(candidate: WorkerCandidate) {
  return (
    candidate.availability === 'ended' ||
    canEndCandidate(candidate)
  )
}

function allocationAction(candidate: WorkerCandidate) {
  if (canHandoffCandidate(candidate)) return 'Hand off worker'
  return candidate.availability === 'resumable'
    ? 'Replace runtime and assign'
    : 'Assign worker'
}

function agentTargetRuntimeMetadata(
  runtime: WorkerRuntimeBinding,
  capabilities: ResolvedRuntimeCapabilities,
  fallbackCwd: string | null = null,
) {
  const observed = capabilities.observedWorker
  const current = observed ?? capabilities.observedPane
  return {
    capabilityReason: capabilities.reason,
    chatAvailable: capabilities.chat,
    cwd:
      current?.foreground_cwd ??
      current?.cwd ??
      fallbackCwd,
    harness:
      observed?.display_provider ??
      current?.provider ??
      runtime.provider_session?.provider ??
      runtime.adapter,
    interactive: capabilities.terminal,
    observation:
      capabilities.reason === 'stale'
        ? ('stale' as const)
        : current
          ? ('observed' as const)
          : ('durable' as const),
    paneId: observed?.pane_id ?? current?.runtime_id ?? runtime.pane_id,
    runtimeAdapter: runtime.adapter,
    status: runtimeCapabilityStatus(runtime, capabilities),
    tabId: current?.tab_id ?? runtime.tab_id,
  }
}

function findCandidateForObservedWorker(
  candidates: WorkerCandidate[],
  inventory: RuntimeInventory | null,
  observed: ObservedWorker,
) {
  if (!inventory) return undefined
  const matches = candidates.filter((candidate) => {
    const runtime = candidate.worker.runtime
    return (
      runtime?.adapter === inventory.adapter &&
      runtime.session === inventory.session &&
      runtime.terminal_id === observed.terminal_id
    )
  })
  return matches.length === 1 ? matches[0] : undefined
}

function projectOrchestratorTransferStatus(
  project: Project,
  reason: ProjectOrchestratorEligibilityReason,
  loading: boolean,
  error: string | null,
) {
  const session = project.runtime.session
  if (loading && reason === 'inventory_unavailable') {
    return `Checking live workers in Herdr session ${session}.`
  }
  if (error && reason === 'inventory_unavailable') {
    return `Could not load Herdr session ${session}: ${error}. Check that the session is running, then refresh.`
  }
  if (reason === 'inventory_identity_changed') {
    return `Live inventory no longer matches Herdr session ${session}. Refresh the session before changing ownership.`
  }
  if (reason === 'project_workspace_unavailable') {
    return 'The project workspace is missing or ambiguous in live inventory. Refresh the session before changing ownership.'
  }
  if (reason === 'current_orchestrator_unavailable') {
    return 'The current orchestrator runtime topology is stale or unavailable. Refresh the session before changing ownership.'
  }
  if (reason === 'no_eligible_workers') {
    return 'Start or free a live worker in this project workspace, then refresh to change ownership.'
  }
  return 'A live unassigned worker is ready to take project ownership.'
}

function resolvedRuntimeState(
  runtime: WorkerRuntimeBinding | null,
  capabilities: ResolvedRuntimeCapabilities,
) {
  return {
    observationState: runtimeCapabilityObservationState(
      capabilities,
      runtime,
    ),
    processState: runtimeCapabilityProcessState(runtime, capabilities),
    status: runtimeCapabilityStatus(runtime, capabilities),
  }
}

function StatusBadge({ status }: { status: ObservedStatus }) {
  const Icon = STATUS_ICONS[status]
  return (
    <span
      aria-label={`Observed status: ${status}`}
      className="status-badge"
      data-status={status}
    >
      <Icon
        aria-hidden="true"
        className={status === 'working' ? 'status-spin' : ''}
        size={14}
      />
      {status}
    </span>
  )
}

const WORKFLOW_STATUS_LABELS: Record<WorkflowStatus, string> = {
  idle: 'Idle',
  needs_attention: 'Needs attention',
  working: 'Working',
}

const WORKFLOW_STATUS_ICONS = {
  idle: Pause,
  needs_attention: CircleAlert,
  working: LoaderCircle,
} satisfies Record<WorkflowStatus, typeof CircleAlert>

function WorkflowStatusSummary({ report }: { report: StatusReport }) {
  const Icon = WORKFLOW_STATUS_ICONS[report.state]
  return (
    <section
      aria-label="Reported workflow status"
      className="workflow-status-summary"
      data-workflow-state={report.state}
    >
      <div className="workflow-status-summary__heading">
        <p className="eyebrow">Reported workflow</p>
        <span className="workflow-status-badge" data-workflow-state={report.state}>
          <Icon
            aria-hidden="true"
            className={report.state === 'working' ? 'status-spin' : ''}
            size={13}
          />
          {WORKFLOW_STATUS_LABELS[report.state]}
        </span>
      </div>
      <dl>
        <div>
          <dt>Last</dt>
          <dd>{report.last || 'Not reported.'}</dd>
        </div>
        <div>
          <dt>Next</dt>
          <dd>{report.next || 'Not reported.'}</dd>
        </div>
      </dl>
      {report.blockers.length > 0 ? (
        <div className="workflow-status-summary__blockers">
          <strong>
            {report.blockers.length} blocker
            {report.blockers.length === 1 ? '' : 's'}
          </strong>
          <span>{report.blockers.join(' / ')}</span>
        </div>
      ) : null}
    </section>
  )
}

function ProcessStateBadge({ state }: { state: RuntimeProcessState }) {
  const Icon = PROCESS_ICONS[state]
  return (
    <span
      aria-label={`Process state: ${state}`}
      className="process-state-badge"
      data-process-state={state}
    >
      <Icon aria-hidden="true" size={14} />
      {state}
    </span>
  )
}

function ObservationStateBadge({
  capabilities,
  state,
}: {
  capabilities: RuntimeCapabilities
  state: RuntimeObservationState
}) {
  const Icon = OBSERVATION_ICONS[state]
  const label = runtimeCapabilityLabel(capabilities)
  const detail = runtimeCapabilityDetail(capabilities)
  return (
    <span
      aria-label={detail ? `${label}. ${detail}` : `Observation state: ${state}`}
      className="observation-state-badge"
      data-observation-state={
        capabilities.reason === 'stale' ? 'stale' : state
      }
      title={detail}
    >
      <Icon aria-hidden="true" size={14} />
      {label}
    </span>
  )
}

function RuntimeStateSummary({
  inventory,
  runtime,
  snapshotCurrent,
}: {
  inventory: RuntimeInventory | null
  runtime: WorkerRuntimeBinding | null
  snapshotCurrent: boolean
}) {
  const capabilities = resolveRuntimeCapabilities(
    snapshotCurrent,
    runtime,
    inventory,
  )
  const { processState, status } = resolvedRuntimeState(
    runtime,
    capabilities,
  )
  const currentObservationState = runtimeCapabilityObservationState(
    capabilities,
    runtime,
  )

  return (
    <dl
      className="runtime-state-summary"
      data-current-observation={
        snapshotCurrent &&
        Boolean(capabilities.observedWorker || capabilities.observedPane)
      }
    >
      <div>
        <dt>Observed status</dt>
        <dd>
          <StatusBadge status={status} />
        </dd>
      </div>
      <div>
        <dt>Process state</dt>
        <dd>
          <ProcessStateBadge state={processState} />
        </dd>
      </div>
      <div>
        <dt>Observation</dt>
        <dd>
          <ObservationStateBadge
            capabilities={capabilities}
            state={currentObservationState}
          />
        </dd>
      </div>
    </dl>
  )
}

function DetailRow({
  label,
  value,
  mono = false,
}: {
  label: string
  value: string | number | null | undefined
  mono?: boolean
}) {
  return (
    <div className="detail-row">
      <dt>{label}</dt>
      <dd className={mono ? 'mono' : ''}>{value ?? 'Not reported'}</dd>
    </div>
  )
}

function ObservedWorkerInspector({ worker }: { worker: ObservedWorker }) {
  return (
    <>
      <div className="inspector__identity">
        <span className="inspector__icon" data-status={worker.status}>
          <Bot aria-hidden="true" size={20} />
        </span>
        <div>
          <p className="eyebrow">Observed worker</p>
          <h2>{workerLabel(worker)}</h2>
        </div>
      </div>
      <StatusBadge status={worker.status} />
      <dl className="detail-list">
        <DetailRow label="Provider" value={worker.provider} />
        <DetailRow label="Workspace" value={worker.workspace_id} mono />
        <DetailRow label="Tab" value={worker.tab_id} mono />
        <DetailRow label="Pane" value={worker.pane_id} mono />
        <DetailRow label="Working directory" value={worker.cwd} mono />
        <DetailRow
          label="Launch readiness"
          value={worker.interactive_ready ? 'Ready' : 'Not ready'}
        />
        <DetailRow
          label="Provider session"
          value={worker.provider_session?.value}
          mono
        />
        <DetailRow
          label="State sequence"
          value={worker.state_change_sequence}
          mono
        />
      </dl>
    </>
  )
}

function ProviderChildInspector({ agent }: { agent: ObservedChildAgent }) {
  const label =
    agent.name ??
    agent.description ??
    `${agent.provider} ${agent.provider_agent_id.slice(0, 8)}`
  return (
    <>
      <div className="inspector__identity">
        <span className="inspector__icon" data-status={agent.status}>
          <GitBranch aria-hidden="true" size={20} />
        </span>
        <div>
          <p className="eyebrow">Provider child agent</p>
          <h2>{label}</h2>
        </div>
      </div>
      <StatusBadge status={agent.status} />
      <dl className="detail-list">
        <DetailRow label="Provider" value={agent.provider} />
        <DetailRow label="Role" value={agent.role} />
        <DetailRow label="Description" value={agent.description} />
        <DetailRow label="Child ID" value={agent.provider_agent_id} mono />
        <DetailRow
          label="Parent session"
          value={agent.parent_provider_session.value}
          mono
        />
        <DetailRow
          label="Parent child"
          value={agent.parent_agent_id}
          mono
        />
        <DetailRow label="Spawn depth" value={agent.depth} />
        <DetailRow label="Terminal" value="Shares parent terminal" />
      </dl>
    </>
  )
}

function WorkerCandidateInspector({
  activeAssignment,
  candidate,
  completedAssignment,
  inventory,
  onAllocate,
  onDelete,
  onEndSession,
  onRefresh,
  projects,
  snapshotCurrent,
}: {
  activeAssignment: Assignment | undefined
  candidate: WorkerCandidate
  completedAssignment: Assignment | undefined
  inventory: RuntimeInventory | null
  onAllocate: (project: Project) => void
  onDelete: () => void
  onEndSession: () => void
  onRefresh: () => void
  projects: Project[]
  snapshotCurrent: boolean
}) {
  const [projectId, setProjectId] = useState(projects[0]?.id ?? '')
  const isHandoff = canHandoffCandidate(candidate)
  const isActionable = canAllocateCandidate(candidate) || isHandoff
  const eligibleProjects = isHandoff
    ? projects.filter((project) => project.id !== candidate.project_id)
    : projects

  useEffect(() => {
    if (!eligibleProjects.some((project) => project.id === projectId)) {
      setProjectId(eligibleProjects[0]?.id ?? '')
    }
  }, [eligibleProjects, projectId])

  const project = eligibleProjects.find(
    (candidate) => candidate.id === projectId,
  )
  const capabilities = resolveRuntimeCapabilities(
    snapshotCurrent,
    candidate.worker.runtime,
    inventory,
  )
  const observed = capabilities.observedWorker
  const current = observed ?? capabilities.observedPane
  const AvailabilityIcon = AVAILABILITY_ICONS[candidate.availability]

  return (
    <>
      <div className="inspector__identity">
        <span
          className="inspector__icon"
          data-availability={candidate.availability}
        >
          <AvailabilityIcon aria-hidden="true" size={20} />
        </span>
        <div>
          <p className="eyebrow">Worker candidate</p>
          <h2>{candidateLabel(candidate)}</h2>
        </div>
      </div>
      <span
        className="availability-badge"
        data-availability={candidate.availability}
      >
        <AvailabilityIcon aria-hidden="true" size={14} />
        {AVAILABILITY_LABELS[candidate.availability]}
      </span>
      <RuntimeStateSummary
        inventory={inventory}
        runtime={candidate.worker.runtime}
        snapshotCurrent={snapshotCurrent}
      />
      <dl className="detail-list">
        <DetailRow label="Worker ID" value={candidate.worker.id} mono />
        <DetailRow label="Profile" value={candidate.profile_name} />
        <DetailRow label="Default role" value={candidate.default_role} />
        <DetailRow
          label="Provider"
          value={observed?.provider ?? current?.provider}
        />
        <DetailRow label="Project ID" value={candidate.project_id} mono />
        <DetailRow
          label="Assignment"
          value={candidate.assignment_id}
          mono
        />
        <DetailRow
          label="Terminal"
          value={candidate.worker.runtime?.terminal_id}
          mono
        />
        <DetailRow
          label="State sequence"
          value={candidate.worker.runtime?.state_change_sequence}
          mono
        />
        <DetailRow
          label="Runtime revision"
          value={candidate.worker.runtime?.revision}
          mono
        />
        <DetailRow label="Reason" value={candidate.reason} />
        <DetailRow
          label="Disposition"
          value={candidate.worker.desired_state}
        />
        <DetailRow
          label="Worker rev"
          value={`v${candidate.worker.version}`}
          mono
        />
      </dl>
      {activeAssignment ? (
        <WorkerInterventions
          key={[
            activeAssignment.id,
            activeAssignment.attempt.id,
            activeAssignment.worker.runtime?.terminal_id,
            activeAssignment.worker.runtime?.pane_id,
            activeAssignment.worker.runtime?.provider_session?.value,
          ].join(':')}
          inventory={inventory}
          onRefresh={onRefresh}
          snapshotCurrent={snapshotCurrent}
          target={{ kind: 'assignment', assignment: activeAssignment }}
        />
      ) : null}
      {completedAssignment &&
      candidate.worker.desired_state === 'running' ? (
        <div className="awaiting-disposition" role="status">
          <CircleCheck aria-hidden="true" size={16} />
          <span>
            <strong>Awaiting disposition</strong>
            <small>
              Work is complete. This session remains available for inspection
              or reassignment until you end it.
            </small>
          </span>
        </div>
      ) : null}
      {isActionable ? (
        <div className="inspector-actions">
          <label className="field-label" htmlFor="worker-allocation-project">
            {isHandoff ? 'Target project' : 'Project'}
          </label>
          <select
            id="worker-allocation-project"
            onChange={(event) => setProjectId(event.target.value)}
            value={projectId}
          >
            {eligibleProjects.map((candidate) => (
              <option key={candidate.id} value={candidate.id}>
                {candidate.name}
              </option>
            ))}
          </select>
          <button
            className="command-button"
            disabled={!project}
            onClick={() => {
              if (project) onAllocate(project)
            }}
            type="button"
          >
            {isHandoff ? (
              <ArrowRightLeft aria-hidden="true" size={16} />
            ) : candidate.availability === 'resumable' ? (
              <RefreshCw aria-hidden="true" size={16} />
            ) : (
              <Plus aria-hidden="true" size={16} />
            )}
            {allocationAction(candidate)}
          </button>
        </div>
      ) : null}
      {canDeleteCandidate(candidate) ? (
        <div className="inspector-actions disposition-actions">
          {canEndCandidate(candidate) ? (
            <button
              className="secondary-button"
              onClick={onEndSession}
              type="button"
            >
              <CircleStop aria-hidden="true" size={16} />
              End session
            </button>
          ) : null}
          <button
            className="destructive-button"
            onClick={onDelete}
            type="button"
          >
            <Trash2 aria-hidden="true" size={16} />
            Delete worker
          </button>
        </div>
      ) : null}
    </>
  )
}

function YardOrchestratorInspector({
  busy,
  inventory,
  onCoordinationChange,
  onProvision,
  onRecover,
  onRefresh,
  orchestrator,
  profiles,
  projects,
  routes,
  sessionRunning,
  snapshotCurrent,
}: {
  busy: boolean
  inventory: RuntimeInventory | null
  onCoordinationChange: (route: YardOrchestratorRoute) => void
  onProvision: (profile: WorkerProfile) => void
  onRecover: () => void
  onRefresh: () => void
  orchestrator: YardOrchestrator
  profiles: WorkerProfile[]
  projects: Project[]
  routes: YardOrchestratorRoute[]
  sessionRunning: boolean | undefined
  snapshotCurrent: boolean
}) {
  const eligibleProfiles = profiles.filter(
    (profile) => profile.runtime_adapter === 'herdr',
  )
  const preferredProfile =
    eligibleProfiles.find((profile) => profile.default_role === 'orchestrator') ??
    eligibleProfiles[0]
  const [profileId, setProfileId] = useState(preferredProfile?.id ?? '')
  const worker = orchestrator.worker
  const capabilities = resolveRuntimeCapabilities(
    snapshotCurrent,
    worker?.runtime ?? null,
    inventory,
  )
  const runtimeState = resolvedRuntimeState(
    worker?.runtime ?? null,
    capabilities,
  )
  const isDedicated =
    worker?.runtime?.session === 'yard-orchestrator'
  const sessionStopped = isDedicated && sessionRunning === false
  const agentStopped =
    isDedicated && runtimeState.processState !== 'running'
  const recoveryRequired = sessionStopped || agentStopped

  useEffect(() => {
    if (!eligibleProfiles.some((profile) => profile.id === profileId)) {
      setProfileId(preferredProfile?.id ?? '')
    }
  }, [eligibleProfiles, preferredProfile?.id, profileId])

  const selectedProfile = eligibleProfiles.find(
    (profile) => profile.id === profileId,
  )

  return (
    <>
      <div className="inspector__identity">
        <span
          className="inspector__icon yard-orchestrator-icon"
          data-status={worker ? runtimeState.status : 'unknown'}
        >
          <Network aria-hidden="true" size={20} />
        </span>
        <div>
          <p className="eyebrow">Portfolio control</p>
          <h2>Superintendent</h2>
        </div>
      </div>
      {worker ? (
        <>
          <div className="yard-control-runtime">
            <StatusBadge status={runtimeState.status} />
            <span>
              <strong>
                {sessionStopped
                  ? 'Dedicated session stopped'
                  : isDedicated
                    ? 'Dedicated Herdr session'
                    : 'Legacy worker binding'}
              </strong>
              <small>
                {worker.runtime?.session ?? 'Runtime unavailable'}
                {worker.runtime?.terminal_id
                  ? ` / ${worker.runtime.terminal_id}`
                  : ''}
              </small>
            </span>
          </div>
          {recoveryRequired ? (
            <div className="inspector-actions yard-orchestrator-recovery">
              <div className="awaiting-disposition" role="status">
                <CircleAlert aria-hidden="true" size={16} />
                <span>
                  <strong>{sessionStopped ? 'Herdr session is offline' : 'Superintendent agent is offline'}</strong>
                  <small>
                    Restart this agent and reconcile it without replacing ownership.
                  </small>
                </span>
              </div>
              <button
                className="command-button"
                disabled={busy}
                onClick={onRecover}
                type="button"
              >
                {busy ? (
                  <LoaderCircle
                    aria-hidden="true"
                    className="status-spin"
                    size={16}
                  />
                ) : (
                  <RefreshCw aria-hidden="true" size={16} />
                )}
                Restart orchestrator session
              </button>
            </div>
          ) : (
            <WorkerInterventions
              key={[
                orchestrator.version,
                worker.id,
                worker.runtime?.terminal_id,
                worker.runtime?.pane_id,
              ].join(':')}
              onCoordinationChange={onCoordinationChange}
              onRefresh={onRefresh}
              inventory={inventory}
              projects={projects}
              routes={routes}
              snapshotCurrent={snapshotCurrent}
              target={{ kind: 'yard-orchestrator', orchestrator }}
            />
          )}
        </>
      ) : (
        <div className="awaiting-disposition" role="status">
          <CircleAlert aria-hidden="true" size={16} />
          <span>
            <strong>Central orchestrator is not running</strong>
            <small>Start a dedicated Herdr session for portfolio coordination.</small>
          </span>
        </div>
      )}
      {!isDedicated ? (
        <div className="inspector-actions yard-orchestrator-configuration">
          <div className="yard-orchestrator-configuration__heading">
            <Server aria-hidden="true" size={17} />
            <span>
              <strong>Dedicated control session</strong>
              <small>Creates a new `yard-orchestrator` Herdr session.</small>
            </span>
          </div>
          <label className="field-label" htmlFor="yard-orchestrator-profile">
            Orchestrator profile
          </label>
          <select
            disabled={busy || eligibleProfiles.length === 0}
            id="yard-orchestrator-profile"
            onChange={(event) => setProfileId(event.target.value)}
            value={profileId}
          >
            {eligibleProfiles.map((profile) => (
              <option key={profile.id} value={profile.id}>
                {profile.name} / {profile.provider}
              </option>
            ))}
          </select>
          <button
            className="command-button"
            disabled={busy || !selectedProfile}
            onClick={() => {
              if (selectedProfile) onProvision(selectedProfile)
            }}
            type="button"
          >
            {busy ? (
              <LoaderCircle
                aria-hidden="true"
                className="status-spin"
                size={16}
              />
            ) : (
              <Network aria-hidden="true" size={16} />
            )}
            {worker ? 'Move to dedicated session' : 'Start Superintendent'}
          </button>
          {eligibleProfiles.length === 0 ? (
            <p className="empty-state">Create a Herdr worker profile first.</p>
          ) : null}
        </div>
      ) : null}
    </>
  )
}

function ProjectOrchestratorTransferDialog({
  busy,
  candidates,
  error,
  onClose,
  onConfirm,
  project,
  returnFocus,
}: {
  busy: boolean
  candidates: WorkerCandidate[]
  error: string | null
  onClose: () => void
  onConfirm: (workerId: string) => Promise<void>
  project: Project
  returnFocus: HTMLElement | null
}) {
  const dialogRef = useRef<HTMLElement>(null)
  const selectRef = useRef<HTMLSelectElement>(null)
  const [workerId, setWorkerId] = useState(
    candidates[0]?.worker.id ?? '',
  )
  const selectedCandidate = candidates.find(
    (candidate) => candidate.worker.id === workerId,
  )
  useModalDialog({
    canClose: !busy,
    dialogRef,
    initialFocusRef: selectRef,
    onClose,
    returnFocus,
  })

  useEffect(() => {
    setWorkerId((current) => {
      if (candidates.length === 0) return current
      return candidates.some((candidate) => candidate.worker.id === current)
        ? current
        : candidates[0].worker.id
    })
  }, [candidates])

  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (workerId) void onConfirm(workerId)
  }

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        aria-labelledby="project-orchestrator-transfer-title"
        aria-modal="true"
        className="control-dialog orchestrator-transfer-dialog"
        ref={dialogRef}
        role="dialog"
        tabIndex={-1}
      >
        <header className="dialog-heading">
          <div>
            <p className="eyebrow">Project ownership</p>
            <h2 id="project-orchestrator-transfer-title">
              Change orchestrator
            </h2>
          </div>
          <button
            aria-label="Close orchestrator change"
            className="icon-button"
            disabled={busy}
            onClick={onClose}
            title="Close"
            type="button"
          >
            <X aria-hidden="true" size={17} />
          </button>
        </header>
        <form className="dialog-form" onSubmit={submit}>
          <label>
            <span>Next orchestrator</span>
            <select
              disabled={busy || candidates.length === 0}
              onChange={(event) => setWorkerId(event.target.value)}
              ref={selectRef}
              value={workerId}
            >
              {candidates.map((candidate) => (
                <option
                  key={candidate.worker.id}
                  value={candidate.worker.id}
                >
                  {candidateLabel(candidate)} -{' '}
                  {candidate.worker.runtime?.terminal_id}
                </option>
              ))}
            </select>
          </label>
          {selectedCandidate ? (
            <div className="ownership-transfer-impact">
              <ArrowRightLeft aria-hidden="true" size={17} />
              <p>
                <span>
                  {`${candidateLabel(selectedCandidate)} takes project orchestration for ${project.name}. The current orchestrator (${project.orchestrator.id}) remains live and becomes unassigned.`}
                </span>
                <small>
                  This transfers ownership only; it does not create or end a
                  runtime.
                </small>
              </p>
            </div>
          ) : (
            <div className="dialog-error" role="alert">
              <CircleAlert aria-hidden="true" size={16} />
              <span>
                No live unassigned worker is currently eligible for this
                project.
              </span>
            </div>
          )}
          {error ? (
            <div className="dialog-error" role="alert">
              <CircleAlert aria-hidden="true" size={16} />
              <span>{error}</span>
            </div>
          ) : null}
          <footer className="dialog-actions">
            <button
              className="secondary-button"
              disabled={busy}
              onClick={onClose}
              type="button"
            >
              Cancel
            </button>
            <button
              className="command-button"
              disabled={busy || !selectedCandidate}
              type="submit"
            >
              {busy ? (
                <LoaderCircle
                  aria-hidden="true"
                  className="status-spin"
                  size={16}
                />
              ) : (
                <ArrowRightLeft aria-hidden="true" size={16} />
              )}
              Change orchestrator
            </button>
          </footer>
        </form>
      </section>
    </div>
  )
}

function ProjectOrchestratorInspector({
  candidates,
  eligibilityReason,
  inventory,
  inventoryError,
  inventoryLoading,
  onChange,
  onRefresh,
  project,
  statusReport,
}: {
  candidates: WorkerCandidate[]
  eligibilityReason: ProjectOrchestratorEligibilityReason
  inventory: RuntimeInventory | null
  inventoryError: string | null
  inventoryLoading: boolean
  onChange: (trigger: HTMLButtonElement) => void
  onRefresh: () => void
  project: Project
  statusReport: StatusReport | undefined
}) {
  const runtime = project.orchestrator.runtime
  const snapshotCurrent = Boolean(inventory && !inventoryError)
  const capabilities = resolveRuntimeCapabilities(
    snapshotCurrent,
    runtime,
    inventory,
  )
  const observed = capabilities.observedWorker
  const runtimeState = resolvedRuntimeState(runtime, capabilities)
  const transferStatusId = `project-orchestrator-transfer-status-${project.id}`

  return (
    <>
      <div className="inspector__identity">
        <span className="inspector__icon" data-status={runtimeState.status}>
          <BriefcaseBusiness aria-hidden="true" size={20} />
        </span>
        <div>
          <p className="eyebrow">Project orchestrator</p>
          <h2>{project.name}</h2>
        </div>
      </div>
      {statusReport ? (
        <WorkflowStatusSummary report={statusReport} />
      ) : null}
      <section
        aria-label="Orchestrator runtime"
        className="durable-runtime-section"
      >
        <p className="eyebrow">Orchestrator runtime</p>
        <RuntimeStateSummary
          inventory={inventory}
          runtime={runtime}
          snapshotCurrent={snapshotCurrent}
        />
        <dl className="detail-list">
          <DetailRow
            label="Worker"
            value={observed ? workerLabel(observed) : 'Durable worker'}
          />
          <DetailRow label="Worker ID" value={project.orchestrator.id} mono />
          <DetailRow label="Herdr session" value={runtime?.session} mono />
          <DetailRow label="Terminal" value={runtime?.terminal_id} mono />
        </dl>
        <button
          aria-describedby={transferStatusId}
          className="secondary-button project-orchestrator-transfer"
          disabled={candidates.length === 0}
          onClick={(event) => onChange(event.currentTarget)}
          type="button"
        >
          <ArrowRightLeft aria-hidden="true" size={15} />
          Change orchestrator
        </button>
        <p
          className="project-orchestrator-transfer-status"
          id={transferStatusId}
          role="status"
          tabIndex={candidates.length === 0 ? 0 : undefined}
        >
          {projectOrchestratorTransferStatus(
            project,
            eligibilityReason,
            inventoryLoading,
            inventoryError,
          )}
        </p>
      </section>
      <WorkerInterventions
        key={[
          project.id,
          project.orchestrator.id,
          runtime?.terminal_id,
          runtime?.pane_id,
        ].join(':')}
        inventory={inventory}
        onRefresh={onRefresh}
        snapshotCurrent={snapshotCurrent}
        target={{ kind: 'orchestrator', project }}
      />
    </>
  )
}

function ProjectInspector({
  accent,
  inventory,
  onAccentChange,
  onArchive,
  onDelete,
  onDisconnect,
  onRefresh,
  project,
  projects,
  relationships,
  snapshotCurrent,
  statusReport,
}: {
  accent: string
  inventory: RuntimeInventory | null
  onAccentChange: (accent: string) => void
  onArchive: (trigger: HTMLButtonElement) => void
  onDelete: (trigger: HTMLButtonElement) => void
  onDisconnect: (relationship: ProjectRelationship) => void
  onRefresh: () => void
  project: Project
  projects: Project[]
  relationships: ProjectRelationship[]
  snapshotCurrent: boolean
  statusReport: StatusReport | undefined
}) {
  const runtimeMatches =
    inventory?.adapter === project.runtime.adapter &&
    inventory.session === project.runtime.session
  const workspace = runtimeMatches
    ? inventory.workspaces.find(
          (candidate) =>
            candidate.runtime_id === project.runtime.workspace_id,
        )
    : undefined
  const orchestratorRuntime = project.orchestrator.runtime
  const orchestratorCapabilities = resolveRuntimeCapabilities(
    snapshotCurrent,
    orchestratorRuntime,
    inventory,
  )
  const orchestrator = orchestratorCapabilities.observedWorker
  const orchestratorState = resolvedRuntimeState(
    orchestratorRuntime,
    orchestratorCapabilities,
  )

  return (
    <>
      <div className="inspector__identity">
        <span
          className="inspector__icon"
          data-status={orchestratorState.status}
        >
          <BriefcaseBusiness aria-hidden="true" size={20} />
        </span>
        <div>
          <p className="eyebrow">Yard project</p>
          <h2>{project.name}</h2>
        </div>
      </div>
      <div className="project-workspace-state">
        <span>Workspace</span>
        {workspace ? (
          <StatusBadge status={workspace.status} />
        ) : (
          <span className="runtime-badge" data-runtime="offline">
            <WifiOff aria-hidden="true" size={14} />
            offline
          </span>
        )}
      </div>
      {statusReport ? (
        <WorkflowStatusSummary report={statusReport} />
      ) : null}
      <dl className="detail-list">
        <DetailRow label="Project ID" value={project.id} mono />
        <DetailRow label="Session" value={project.runtime.session} mono />
        <DetailRow
          label="Workspace"
          value={project.runtime.workspace_id}
          mono
        />
        <DetailRow
          label="Placement"
          value={`v${project.placement.version}`}
          mono
        />
      </dl>
      {relationships.length > 0 ? (
        <section
          aria-label="Project connections"
          className="project-connections"
        >
          <p className="eyebrow">Connections</p>
          {relationships.map((relationship) => {
            const outgoing =
              relationship.source_project_id === project.id
            const otherProject = projects.find(
              (candidate) =>
                candidate.id ===
                (outgoing
                  ? relationship.target_project_id
                  : relationship.source_project_id),
            )
            return (
              <div
                className="project-connection-row"
                key={relationship.id}
              >
                <span>
                  <strong>{otherProject?.name ?? 'Unknown project'}</strong>
                  <small>
                    {outgoing ? 'depends on' : 'required by'}
                  </small>
                </span>
                <button
                  aria-label={`Remove connection with ${otherProject?.name ?? 'project'}`}
                  className="icon-button"
                  onClick={() => onDisconnect(relationship)}
                  title="Remove project connection"
                  type="button"
                >
                  <Unlink aria-hidden="true" size={15} />
                </button>
              </div>
            )
          })}
        </section>
      ) : null}
      <fieldset className="project-color-control">
        <legend>Territory border</legend>
        <div aria-label="Project border color" className="color-swatches">
          {PROJECT_ACCENTS.map((option) => (
            <button
              aria-label={option.label}
              aria-pressed={accent === option.value}
              key={option.value}
              onClick={() => onAccentChange(option.value)}
              style={{ '--swatch': option.value } as CSSProperties}
              title={option.label}
              type="button"
            />
          ))}
        </div>
      </fieldset>
      <section
        aria-label="Orchestrator runtime"
        className="durable-runtime-section"
      >
        <p className="eyebrow">Orchestrator runtime</p>
        <RuntimeStateSummary
          inventory={inventory}
          runtime={orchestratorRuntime}
          snapshotCurrent={snapshotCurrent}
        />
        <dl className="detail-list">
          <DetailRow
            label="Orchestrator"
            value={orchestrator ? workerLabel(orchestrator) : 'Durable worker'}
          />
          <DetailRow
            label="Worker ID"
            value={project.orchestrator.id}
            mono
          />
          <DetailRow
            label="Terminal"
            value={orchestratorRuntime?.terminal_id}
            mono
          />
          <DetailRow
            label="State sequence"
            value={orchestratorRuntime?.state_change_sequence}
            mono
          />
          <DetailRow
            label="Runtime revision"
            value={orchestratorRuntime?.revision}
            mono
          />
        </dl>
      </section>
      <WorkerInterventions
        key={[
          project.id,
          project.orchestrator.id,
          orchestratorRuntime?.terminal_id,
          orchestratorRuntime?.pane_id,
          orchestratorRuntime?.provider_session?.value,
        ].join(':')}
        inventory={inventory}
        onRefresh={onRefresh}
        snapshotCurrent={snapshotCurrent}
        target={{ kind: 'orchestrator', project }}
      />
      <div className="inspector-actions disposition-actions">
        <button
          className="secondary-button"
          onClick={(event) => onArchive(event.currentTarget)}
          type="button"
        >
          <FolderArchive aria-hidden="true" size={16} />
          Archive project
        </button>
        <button
          className="destructive-button"
          onClick={(event) => onDelete(event.currentTarget)}
          type="button"
        >
          <Trash2 aria-hidden="true" size={16} />
          Delete project
        </button>
      </div>
    </>
  )
}

function ProfileInspector({
  onAllocate,
  onEdit,
  profile,
  projects,
}: {
  onAllocate: (project: Project) => void
  onEdit: () => void
  profile: WorkerProfile
  projects: Project[]
}) {
  const [projectId, setProjectId] = useState(projects[0]?.id ?? '')

  useEffect(() => {
    if (!projects.some((project) => project.id === projectId)) {
      setProjectId(projects[0]?.id ?? '')
    }
  }, [projectId, projects])

  const project = projects.find((candidate) => candidate.id === projectId)

  return (
    <>
      <div className="inspector__identity">
        <span className="inspector__icon profile-icon">
          <Bot aria-hidden="true" size={20} />
        </span>
        <div>
          <p className="eyebrow">Worker profile</p>
          <h2>{profile.name}</h2>
        </div>
      </div>
      <span className="runtime-badge">
        {profile.provider}
        {profile.model ? ` / ${profile.model}` : ''}
      </span>
      <dl className="detail-list">
        <DetailRow label="Default role" value={profile.default_role} />
        <DetailRow label="Runtime" value={profile.runtime_adapter} mono />
        <DetailRow label="Worktree" value={profile.worktree_policy} />
        <DetailRow label="Sandbox" value={profile.sandbox_policy} />
        <DetailRow label="Permissions" value={profile.permission_policy} />
        <DetailRow label="Revision" value={`v${profile.version}`} mono />
      </dl>
      <div className="inspector-actions">
        <button className="secondary-button" onClick={onEdit} type="button">
          <Pencil aria-hidden="true" size={15} />
          Edit profile
        </button>
        <label className="field-label" htmlFor="allocation-project">
          Allocate to project
        </label>
        <select
          id="allocation-project"
          onChange={(event) => setProjectId(event.target.value)}
          value={projectId}
        >
          {projects.map((candidate) => (
            <option key={candidate.id} value={candidate.id}>
              {candidate.name}
            </option>
          ))}
        </select>
        <button
          className="command-button"
          disabled={!project}
          onClick={() => {
            if (project) onAllocate(project)
          }}
          type="button"
        >
          <Plus aria-hidden="true" size={16} />
          Allocate worker
        </button>
      </div>
    </>
  )
}

function AssignmentInspector({
  assignment,
  inventory,
  onDelete,
  onEndSession,
  onRecordCompletion,
  onRefresh,
  snapshotCurrent,
}: {
  assignment: Assignment
  inventory: RuntimeInventory | null
  onDelete?: () => void
  onEndSession?: () => void
  onRecordCompletion: () => void
  onRefresh: () => void
  snapshotCurrent: boolean
}) {
  const [selectedArtifact, setSelectedArtifact] = useState<Artifact | null>(null)
  const artifactTrigger = useRef<HTMLButtonElement | null>(null)
  const runtime = assignment.worker.runtime
  const capabilities = resolveRuntimeCapabilities(
    snapshotCurrent,
    runtime,
    inventory,
  )
  const { status } = resolvedRuntimeState(runtime, capabilities)
  const receipt = assignment.completion_receipt

  return (
    <>
      <div className="inspector__identity">
        <span className="inspector__icon" data-status={status}>
          <Bot aria-hidden="true" size={20} />
        </span>
        <div>
          <p className="eyebrow">Assignment</p>
          <h2>{assignment.profile_name}</h2>
        </div>
      </div>
      <span className="runtime-badge">{assignment.lifecycle}</span>
      <RuntimeStateSummary
        inventory={inventory}
        runtime={runtime}
        snapshotCurrent={snapshotCurrent}
      />
      <dl className="detail-list">
        <DetailRow label="Role" value={assignment.role} />
        <DetailRow label="Objective" value={assignment.objective} />
        <DetailRow label="Attempt" value={assignment.attempt.lifecycle} />
        <DetailRow label="Worker ID" value={assignment.worker.id} mono />
        <DetailRow label="Terminal" value={runtime?.terminal_id} mono />
        <DetailRow label="Tab" value={runtime?.tab_id} mono />
        <DetailRow
          label="State sequence"
          value={runtime?.state_change_sequence}
          mono
        />
        <DetailRow label="Runtime revision" value={runtime?.revision} mono />
        <DetailRow
          label="Profile rev"
          value={`v${assignment.profile_version}`}
          mono
        />
        {assignment.attempt.error ? (
          <DetailRow label="Failure" value={assignment.attempt.error} />
        ) : null}
      </dl>
      <WorkerInterventions
        key={[
          assignment.id,
          assignment.attempt.id,
          runtime?.terminal_id,
          runtime?.pane_id,
          runtime?.provider_session?.value,
        ].join(':')}
        inventory={inventory}
        onRefresh={onRefresh}
        snapshotCurrent={snapshotCurrent}
        target={{ kind: 'assignment', assignment }}
      />
      {assignment.lifecycle === 'active' && status === 'done' ? (
        <div className="awaiting-disposition" role="status">
          <CircleCheck aria-hidden="true" size={16} />
          <span>
            <strong>Completion review</strong>
            <small>Agent reports done. Evidence-backed receipt required.</small>
          </span>
        </div>
      ) : null}
      {receipt ? (
        <section
          aria-label="Completion receipt"
          className="completion-receipt"
        >
          <p className="eyebrow">Completion receipt</p>
          <dl className="detail-list">
            <DetailRow label="Summary" value={receipt.summary} />
            <DetailRow label="Outcome" value={receipt.outcome} />
            <div className="detail-row">
              <dt>Evidence</dt>
              <dd>
                <ReceiptValues values={receipt.evidence_refs} />
              </dd>
            </div>
            <div className="detail-row">
              <dt>Artifacts</dt>
              <dd>
                <ReceiptArtifacts
                  artifacts={receipt.artifacts}
                  onOpen={(artifact, trigger) => {
                    artifactTrigger.current = trigger
                    setSelectedArtifact(artifact)
                  }}
                />
              </dd>
            </div>
            <div className="detail-row">
              <dt>References</dt>
              <dd>
                <ReceiptValues values={receipt.artifact_refs} />
              </dd>
            </div>
            <div className="detail-row">
              <dt>Blockers</dt>
              <dd>
                <ReceiptValues values={receipt.unresolved_blockers} />
              </dd>
            </div>
            <DetailRow label="Actor" value={receipt.actor} mono />
            <DetailRow
              label="Recorded"
              value={new Date(receipt.created_at_unix_ms).toISOString()}
              mono
            />
          </dl>
        </section>
      ) : null}
      {receipt && (onEndSession || onDelete) ? (
        <div className="inspector-actions disposition-actions">
          <div className="awaiting-disposition" role="status">
            <CircleCheck aria-hidden="true" size={16} />
            <span>
              <strong>Awaiting disposition</strong>
              <small>
                The completion receipt is retained whether this session is
                reassigned or ended.
              </small>
            </span>
          </div>
          {onEndSession ? (
            <button
              className="secondary-button"
              onClick={onEndSession}
              type="button"
            >
              <CircleStop aria-hidden="true" size={16} />
              End session
            </button>
          ) : null}
          {onDelete ? (
            <button
              className="destructive-button"
              onClick={onDelete}
              type="button"
            >
              <Trash2 aria-hidden="true" size={16} />
              Delete worker
            </button>
          ) : null}
        </div>
      ) : null}
      {assignment.lifecycle === 'active' ? (
        <div className="inspector-actions">
          <small className="inspector-action-note">
            Record completion before ending or deleting this worker.
          </small>
          <button
            className="command-button"
            onClick={onRecordCompletion}
            type="button"
          >
            <CircleCheck aria-hidden="true" size={16} />
            Record completion
          </button>
        </div>
      ) : null}
      {selectedArtifact ? (
        <Suspense fallback={null}>
          <ArtifactInspector
            artifact={selectedArtifact}
            onClose={() => setSelectedArtifact(null)}
            returnFocus={artifactTrigger.current}
          />
        </Suspense>
      ) : null}
    </>
  )
}

function ReceiptArtifacts({
  artifacts,
  onOpen,
}: {
  artifacts: Artifact[]
  onOpen: (artifact: Artifact, trigger: HTMLButtonElement) => void
}) {
  if (artifacts.length === 0) {
    return <span className="receipt-values__empty">None</span>
  }

  return (
    <ul className="receipt-artifacts">
      {artifacts.map((artifact) => (
        <li key={artifact.id}>
          <button
            onClick={(event) => onOpen(artifact, event.currentTarget)}
            type="button"
          >
            <FileCode2 aria-hidden="true" size={15} />
            <span>
              <strong>{artifact.display_name}</strong>
              <small>{artifact.media_type}</small>
            </span>
          </button>
        </li>
      ))}
    </ul>
  )
}

function ReceiptValues({ values }: { values: string[] }) {
  if (values.length === 0) {
    return <span className="receipt-values__empty">None</span>
  }

  return (
    <ul className="receipt-values">
      {values.map((value, index) => (
        <li key={`${index}:${value}`}>{value}</li>
      ))}
    </ul>
  )
}

function NewProjectInspector({
  busy,
  onCreate,
  onNewProfile,
  profiles,
  session,
}: {
  busy: boolean
  onCreate: (details: WorkspaceProjectCreationDetails) => Promise<void>
  onNewProfile: () => void
  profiles: WorkerProfile[]
  session: string
}) {
  const [name, setName] = useState('')
  const [cwd, setCwd] = useState('')
  const [profileId, setProfileId] = useState(profiles[0]?.id ?? '')
  const [objective, setObjective] = useState('')
  const commandId = useRef(crypto.randomUUID())

  useEffect(() => {
    if (!profiles.some((profile) => profile.id === profileId)) {
      setProfileId(profiles[0]?.id ?? '')
      commandId.current = crypto.randomUUID()
    }
  }, [profileId, profiles])

  const updateCommand = () => {
    commandId.current = crypto.randomUUID()
  }
  const submit = (event: FormEvent) => {
    event.preventDefault()
    void onCreate({
      commandId: commandId.current,
      cwd,
      name,
      objective,
      profileId,
    })
  }

  return (
    <>
      <div className="inspector__identity">
        <span className="inspector__icon project-bootstrap__icon">
          <FolderPlus aria-hidden="true" size={20} />
        </span>
        <div>
          <p className="eyebrow">New workspace</p>
          <h2>Create project</h2>
        </div>
      </div>
      <dl className="detail-list">
        <DetailRow label="Herdr session" value={session || 'Unavailable'} mono />
      </dl>
      {profiles.length > 0 ? (
        <form
          className="project-adoption-form project-bootstrap-form"
          onSubmit={submit}
        >
          <label className="field-label" htmlFor="new-project-name">
            Project name
          </label>
          <input
            autoComplete="off"
            id="new-project-name"
            maxLength={120}
            onChange={(event) => {
              setName(event.target.value)
              updateCommand()
            }}
            required
            value={name}
          />
          <label className="field-label" htmlFor="new-project-cwd">
            Checkout path
          </label>
          <input
            autoComplete="off"
            id="new-project-cwd"
            maxLength={4096}
            onChange={(event) => {
              setCwd(event.target.value)
              updateCommand()
            }}
            placeholder="/path/to/project"
            required
            value={cwd}
          />
          <label className="field-label" htmlFor="new-project-profile">
            Orchestrator profile
          </label>
          <select
            id="new-project-profile"
            onChange={(event) => {
              setProfileId(event.target.value)
              updateCommand()
            }}
            required
            value={profileId}
          >
            {profiles.map((profile) => (
              <option key={profile.id} value={profile.id}>
                {profile.name}
              </option>
            ))}
          </select>
          <label className="field-label" htmlFor="new-project-objective">
            Orchestrator brief
          </label>
          <textarea
            id="new-project-objective"
            maxLength={16000}
            onChange={(event) => {
              setObjective(event.target.value)
              updateCommand()
            }}
            required
            rows={5}
            value={objective}
          />
          <button
            className="command-button"
            disabled={
              busy ||
              !session ||
              !name.trim() ||
              !cwd.trim() ||
              !profileId ||
              !objective.trim()
            }
            type="submit"
          >
            {busy ? (
              <LoaderCircle
                aria-hidden="true"
                className="status-spin"
                size={16}
              />
            ) : (
              <FolderPlus aria-hidden="true" size={16} />
            )}
            Create project
          </button>
        </form>
      ) : (
        <div className="project-bootstrap__empty">
          <p>No orchestrator profiles.</p>
          <button
            className="secondary-button"
            onClick={onNewProfile}
            type="button"
          >
            <Plus aria-hidden="true" size={15} />
            New profile
          </button>
        </div>
      )}
    </>
  )
}

function WorkspaceInspector({
  busy,
  onCreate,
  profiles,
  workers,
  workspace,
}: {
  busy: boolean
  onCreate: (details: ProjectCreationDetails) => Promise<void>
  profiles: WorkerProfile[]
  workers: ObservedWorker[]
  workspace: WorkspaceObservation
}) {
  const [name, setName] = useState(workspace.label)
  const [mode, setMode] = useState<'existing' | 'profile'>(
    workers.length > 0 ? 'existing' : 'profile',
  )
  const [orchestratorId, setOrchestratorId] = useState(
    workers[0]?.runtime_id ?? '',
  )
  const [profileId, setProfileId] = useState(profiles[0]?.id ?? '')
  const [objective, setObjective] = useState(
    `Coordinate work for ${workspace.label}.`,
  )
  const profileCommandId = useRef(crypto.randomUUID())

  useEffect(() => {
    if (!workers.some((worker) => worker.runtime_id === orchestratorId)) {
      setOrchestratorId(workers[0]?.runtime_id ?? '')
      if (workers.length === 0) setMode('profile')
    }
  }, [orchestratorId, workers])

  useEffect(() => {
    if (!profiles.some((profile) => profile.id === profileId)) {
      setProfileId(profiles[0]?.id ?? '')
      if (profiles.length === 0 && workers.length > 0) setMode('existing')
    }
  }, [profileId, profiles, workers.length])

  const submit = (event: FormEvent) => {
    event.preventDefault()
    void onCreate(
      mode === 'existing'
        ? { mode, name, orchestratorId }
        : {
            commandId: profileCommandId.current,
            mode,
            name,
            objective,
            profileId,
          },
    )
  }

  return (
    <>
      <div className="inspector__identity">
        <span className="inspector__icon" data-status={workspace.status}>
          <Boxes aria-hidden="true" size={20} />
        </span>
        <div>
          <p className="eyebrow">Observed workspace</p>
          <h2>{workspace.label}</h2>
        </div>
      </div>
      <StatusBadge status={workspace.status} />
      <dl className="detail-list">
        <DetailRow label="Runtime ID" value={workspace.runtime_id} mono />
        <DetailRow label="Active tab" value={workspace.active_tab_id} mono />
        <DetailRow label="Tabs" value={workspace.tab_count} />
        <DetailRow label="Panes" value={workspace.pane_count} />
        <DetailRow
          label="Repository"
          value={workspace.worktree?.repository_name}
        />
        <DetailRow
          label="Checkout"
          value={workspace.worktree?.checkout_path}
          mono
        />
      </dl>
      <form className="project-adoption-form" onSubmit={submit}>
        <p className="eyebrow">Create project</p>
        <label className="field-label" htmlFor="project-name">
          Project name
        </label>
        <input
          autoComplete="off"
          id="project-name"
          maxLength={120}
          onChange={(event) => {
            setName(event.target.value)
            profileCommandId.current = crypto.randomUUID()
          }}
          required
          value={name}
        />
        <div
          aria-label="Orchestrator source"
          className="segmented-control project-creation-mode"
          role="group"
        >
          <button
            aria-pressed={mode === 'existing'}
            className={mode === 'existing' ? 'is-active' : ''}
            disabled={workers.length === 0}
            onClick={() => setMode('existing')}
            type="button"
          >
            Existing
          </button>
          <button
            aria-pressed={mode === 'profile'}
            className={mode === 'profile' ? 'is-active' : ''}
            disabled={profiles.length === 0}
            onClick={() => setMode('profile')}
            type="button"
          >
            Profile
          </button>
        </div>
        {mode === 'existing' ? (
          <>
            <label className="field-label" htmlFor="orchestrator-select">
              Orchestrator
            </label>
            <select
              disabled={workers.length === 0}
              id="orchestrator-select"
              onChange={(event) => setOrchestratorId(event.target.value)}
              required
              value={orchestratorId}
            >
              {workers.map((worker) => (
                <option key={worker.runtime_id} value={worker.runtime_id}>
                  {workerLabel(worker)}
                </option>
              ))}
            </select>
          </>
        ) : (
          <>
            <label className="field-label" htmlFor="orchestrator-profile">
              Worker profile
            </label>
            <select
              disabled={profiles.length === 0}
              id="orchestrator-profile"
              onChange={(event) => {
                setProfileId(event.target.value)
                profileCommandId.current = crypto.randomUUID()
              }}
              required
              value={profileId}
            >
              {profiles.map((profile) => (
                <option key={profile.id} value={profile.id}>
                  {profile.name}
                </option>
              ))}
            </select>
            <label className="field-label" htmlFor="orchestrator-objective">
              Orchestrator objective
            </label>
            <textarea
              id="orchestrator-objective"
              maxLength={16000}
              onChange={(event) => {
                setObjective(event.target.value)
                profileCommandId.current = crypto.randomUUID()
              }}
              required
              rows={4}
              value={objective}
            />
          </>
        )}
        <button
          className="command-button"
          disabled={
            busy ||
            !name.trim() ||
            (mode === 'existing'
              ? !orchestratorId
              : !profileId || !objective.trim())
          }
          type="submit"
        >
          {busy ? (
            <LoaderCircle
              aria-hidden="true"
              className="status-spin"
              size={16}
            />
          ) : (
            <FolderPlus aria-hidden="true" size={16} />
          )}
          Create project
        </button>
      </form>
    </>
  )
}

function App() {
  const [theme, setTheme] = useState<ThemeId>(readTheme)
  const [mapVisualMode, setMapVisualMode] =
    useState<MapVisualMode>(readMapVisualMode)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [sessions, setSessions] = useState<RuntimeSession[]>([])
  const [selectedSession, setSelectedSession] = useState('')
  const [inventory, setInventory] = useState<RuntimeInventory | null>(null)
  const [inventoryCurrent, setInventoryCurrent] = useState(false)
  const [runtimeTopology, setRuntimeTopology] =
    useState<RuntimeTopology | null>(null)
  const [projectTransferContexts, setProjectTransferContexts] = useState<
    Record<string, ProjectTransferContextEntry>
  >({})
  const [yardOrchestrator, setYardOrchestrator] =
    useState<YardOrchestrator | null>(null)
  const [projects, setProjects] = useState<Project[]>([])
  const [projectRelationships, setProjectRelationships] = useState<
    ProjectRelationship[]
  >([])
  const [yardOrchestratorRoutes, setYardOrchestratorRoutes] = useState<
    YardOrchestratorRoute[]
  >([])
  const [coordinationNodes, setCoordinationNodes] = useState<
    CoordinationNode[]
  >([])
  const [coordinationNodeRoutes, setCoordinationNodeRoutes] = useState<
    CoordinationNodeRoute[]
  >([])
  const [coordinationSnapshots, setCoordinationSnapshots] = useState<
    Record<string, CoordinationSnapshot[]>
  >({})
  const [automations, setAutomations] = useState<Automation[]>([])
  const [tokenSpendSettings, setTokenSpendSettings] =
    useState<TokenSpendSettings | null>(null)
  const [tokenSpendSettingsOpen, setTokenSpendSettingsOpen] = useState(false)
  const [tokenSpendSettingsBusy, setTokenSpendSettingsBusy] = useState(false)
  const [tokenSpendSettingsError, setTokenSpendSettingsError] = useState<
    string | null
  >(null)
  const [orchestratorWorkflowProfile, setOrchestratorWorkflowProfile] =
    useState<OrchestratorWorkflowProfile | null>(null)
  const [orchestratorWorkflowOpen, setOrchestratorWorkflowOpen] =
    useState(false)
  const [orchestratorWorkflowBusy, setOrchestratorWorkflowBusy] =
    useState(false)
  const [orchestratorWorkflowError, setOrchestratorWorkflowError] = useState<
    string | null
  >(null)
  const [automationRuns, setAutomationRuns] = useState<
    Record<string, AutomationRun[]>
  >({})
  const [profiles, setProfiles] = useState<WorkerProfile[]>([])
  const [workerCandidates, setWorkerCandidates] = useState<
    WorkerCandidate[]
  >([])
  const [assignments, setAssignments] = useState<Assignment[]>([])
  const [projectStatusReports, setProjectStatusReports] =
    useState<ProjectStatusReports>({})
  const [projectPulseOpen, setProjectPulseOpen] = useState(false)
  const [filter, setFilter] = useState<Filter>('current')
  const [railView, setRailView] = useState<RailView>('profiles')
  const [resourceShelfOpen, setResourceShelfOpen] = useState(true)
  const [selection, setSelection] = useState<CanvasSelection>(null)
  const [agentWorkspaceMode, setAgentWorkspaceMode] =
    useState<AgentWorkspaceView>('map')
  const [terminalPresentation, setTerminalPresentation] =
    useState<TerminalPresentation>(DEFAULT_TERMINAL_PRESENTATION)
  const [agentWorkspaceTarget, setAgentWorkspaceTarget] =
    useState<AgentWorkspaceTarget | null>(null)
  const [projectAccents, setProjectAccents] = useState<
    Record<string, string>
  >(readProjectAccents)
  const [runtimeLoading, setRuntimeLoading] = useState(true)
  const [projectLoading, setProjectLoading] = useState(true)
  const [yardOrchestratorBusy, setYardOrchestratorBusy] = useState(false)
  const [coordinationNodeBusy, setCoordinationNodeBusy] = useState(false)
  const [coordinationNodeError, setCoordinationNodeError] = useState<
    string | null
  >(null)
  const [coordinationNodePlacement, setCoordinationNodePlacement] =
    useState<CanvasPlacement | null>(null)
  const [coordinationNodeInitialKind, setCoordinationNodeInitialKind] =
    useState<CoordinationNodeKind>('workstream')
  const [automationCreation, setAutomationCreation] = useState<{
    initialScope?: AutomationScope
    placement: CanvasPlacement
  } | null>(null)
  const [automationBusy, setAutomationBusy] = useState(false)
  const [automationHistoryLoading, setAutomationHistoryLoading] =
    useState(false)
  const [automationError, setAutomationError] = useState<string | null>(null)
  const [adoptingWorkspace, setAdoptingWorkspace] = useState<string | null>(
    null,
  )
  const [workspaceProjectBusy, setWorkspaceProjectBusy] = useState(false)
  const [workspaceProjectOpen, setWorkspaceProjectOpen] = useState(false)
  const [profileEditor, setProfileEditor] = useState<
    WorkerProfile | null | undefined
  >(undefined)
  const [profileSaving, setProfileSaving] = useState(false)
  const [allocationProposal, setAllocationProposal] = useState<{
    subject: AllocationSubject
    project: Project
    commandId: string
  } | null>(null)
  const [allocationBusy, setAllocationBusy] = useState(false)
  const [allocationError, setAllocationError] = useState<string | null>(null)
  const [handoffProposal, setHandoffProposal] = useState<{
    commandId: string
    sourceAssignment: Assignment
    sourceProject: Project
    targetProject: Project
  } | null>(null)
  const [handoffBusy, setHandoffBusy] = useState(false)
  const [handoffError, setHandoffError] = useState<string | null>(null)
  const [projectOrchestratorTransfer, setProjectOrchestratorTransfer] =
    useState<{
      commandId: string
      projectId: string
      returnFocus: HTMLElement | null
    } | null>(null)
  const [projectOrchestratorTransferBusy, setProjectOrchestratorTransferBusy] =
    useState(false)
  const [projectOrchestratorTransferError, setProjectOrchestratorTransferError] =
    useState<string | null>(null)
  const [completionProposal, setCompletionProposal] = useState<{
    assignment: Assignment
    commandId: string
  } | null>(null)
  const [completionBusy, setCompletionBusy] = useState(false)
  const [completionError, setCompletionError] = useState<string | null>(null)
  const [endSessionProposal, setEndSessionProposal] = useState<{
    candidate: WorkerCandidate
    commandId: string
  } | null>(null)
  const [endSessionBusy, setEndSessionBusy] = useState(false)
  const [endSessionError, setEndSessionError] = useState<string | null>(null)
  const [workerDeleteProposal, setWorkerDeleteProposal] = useState<{
    candidate: WorkerCandidate
    deleteCommandId: string
    endCommandId: string
  } | null>(null)
  const [workerDeleteBusy, setWorkerDeleteBusy] = useState(false)
  const [workerDeleteError, setWorkerDeleteError] = useState<string | null>(
    null,
  )
  const [projectArchiveProposal, setProjectArchiveProposal] = useState<{
    commandId: string
    project: Project
    returnFocus: HTMLButtonElement | null
  } | null>(null)
  const [projectArchiveBusy, setProjectArchiveBusy] = useState(false)
  const [projectArchiveError, setProjectArchiveError] = useState<
    string | null
  >(null)
  const [projectDeleteProposal, setProjectDeleteProposal] = useState<{
    archiveCommandId: string
    deleteCommandId: string
    project: Project
    returnFocus: HTMLButtonElement | null
  } | null>(null)
  const [projectDeleteBusy, setProjectDeleteBusy] = useState(false)
  const [projectDeleteError, setProjectDeleteError] = useState<string | null>(
    null,
  )
  const [runtimeError, setRuntimeError] = useState<string | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)
  const [actionNotice, setActionNotice] = useState<string | null>(null)
  const placementUpdates = useRef(new Set<string>())
  const pendingPlacements = useRef(new Map<string, CanvasPlacement>())
  const projectsRef = useRef<Project[]>([])
  const projectTransferContextsRef = useRef<
    Record<string, ProjectTransferContextEntry>
  >({})
  const projectTransferGenerations = useRef(new Map<string, number>())
  const projectOrchestratorTransferBusyRef = useRef(false)
  const coordinationNodesRef = useRef<CoordinationNode[]>([])
  const coordinationPlacementUpdates = useRef(new Set<string>())
  const pendingCoordinationPlacements = useRef(
    new Map<string, CanvasPlacement>(),
  )
  const automationsRef = useRef<Automation[]>([])
  const automationPlacementUpdates = useRef(new Set<string>())
  const pendingAutomationPlacements = useRef(
    new Map<string, CanvasPlacement>(),
  )
  const projectPulseTrigger = useRef<HTMLButtonElement | null>(null)
  const settingsTrigger = useRef<HTMLButtonElement | null>(null)

  useEffect(() => {
    applyTheme(theme)
  }, [theme])

  useEffect(() => {
    writeMapVisualMode(mapVisualMode)
  }, [mapVisualMode])

  useEffect(() => {
    projectsRef.current = projects
  }, [projects])

  useEffect(() => {
    coordinationNodesRef.current = coordinationNodes
  }, [coordinationNodes])

  useEffect(() => {
    automationsRef.current = automations
  }, [automations])

  const loadSessions = useCallback(async (signal?: AbortSignal) => {
    const result = await fetchSessions(signal)
    setSessions(result.sessions)
    setSelectedSession((current) => {
      if (
        current &&
        result.sessions.some(
          (session) => session.name === current && session.running,
        )
      ) {
        return current
      }
      return (
        result.sessions.find(
          (session) => session.is_default && session.running,
        )?.name ??
        result.sessions.find((session) => session.running)?.name ??
        ''
      )
    })
    return result.sessions
  }, [])

  const loadAssignments = useCallback(
    async (projectIds: string[], signal?: AbortSignal) => {
      const assignmentResults = await Promise.all(
        projectIds.map((projectId) =>
          fetchProjectAssignments(projectId, signal),
        ),
      )
      const loadedAssignments = assignmentResults.flatMap(
        (result) => result.assignments,
      )
      setAssignments((current) =>
        reconcileRuntimeProjectionSnapshot(current, loadedAssignments),
      )
      return loadedAssignments
    },
    [],
  )

  const loadProjects = useCallback(
    async (signal?: AbortSignal) => {
      const result = await fetchProjects(signal)
      setProjects(result.projects)
      return loadAssignments(
        result.projects.map((project) => project.id),
        signal,
      )
    },
    [loadAssignments],
  )

  const loadYardOrchestrator = useCallback(
    async (signal?: AbortSignal) => {
      const result = await fetchYardOrchestrator(signal)
      setYardOrchestrator((current) =>
        reconcileRuntimeProjectionSnapshot(current, result),
      )
      return result
    },
    [],
  )

  const loadAutomations = useCallback(async (signal?: AbortSignal) => {
    const result = await fetchAutomations(signal)
    setAutomations(result.automations)
    return result.automations
  }, [])

  const loadTokenSpendSettings = useCallback(async (signal?: AbortSignal) => {
    const result = await fetchTokenSpendSettings(signal)
    setTokenSpendSettings(result)
    return result
  }, [])

  const loadOrchestratorWorkflowProfile = useCallback(
    async (signal?: AbortSignal) => {
      const result = await fetchOrchestratorWorkflowProfile(signal)
      setOrchestratorWorkflowProfile(result)
      return result
    },
    [],
  )

  const loadAutomationRuns = useCallback(
    async (automationId: string, signal?: AbortSignal) => {
      const result = await fetchAutomationRuns(automationId, signal)
      setAutomationRuns((current) => ({
        ...current,
        [automationId]: result.runs,
      }))
      return result.runs
    },
    [],
  )

  const loadCoordination = useCallback(
    async (signal?: AbortSignal) => {
      const [relationshipResult, routeResult, nodeResult] = await Promise.all([
        fetchProjectRelationships(signal),
        fetchYardOrchestratorRoutes(100, signal),
        fetchCoordinationNodes(signal),
      ])
      const nodeDetails = await Promise.all(
        nodeResult.nodes.map(async (node) => {
          const [routes, snapshots] = await Promise.all([
            node.kind === 'workstream'
              ? fetchCoordinationNodeRoutes(node.id, 100, signal)
              : Promise.resolve({ routes: [] }),
            node.kind === 'knowledge_store'
              ? fetchCoordinationSnapshots(node.id, signal)
              : Promise.resolve({ snapshots: [] }),
          ])
          return { node, routes: routes.routes, snapshots: snapshots.snapshots }
        }),
      )
      setProjectRelationships(relationshipResult.relationships)
      setYardOrchestratorRoutes(routeResult.routes)
      setCoordinationNodes(nodeResult.nodes)
      setCoordinationNodeRoutes(
        nodeDetails.flatMap((details) => details.routes),
      )
      setCoordinationSnapshots(
        Object.fromEntries(
          nodeDetails.map((details) => [
            details.node.id,
            details.snapshots,
          ]),
        ),
      )
    },
    [],
  )

  const loadProfiles = useCallback(async (signal?: AbortSignal) => {
    const result = await fetchWorkerProfiles(signal)
    setProfiles(result.profiles)
  }, [])

  const loadWorkers = useCallback(async (signal?: AbortSignal) => {
    const result = await fetchWorkers(signal)
    setWorkerCandidates((current) =>
      reconcileRuntimeProjectionSnapshot(current, result.workers),
    )
    return result.workers
  }, [])

  const loadInventory = useCallback(
    async (
      session: string,
      signal?: AbortSignal,
      background = false,
    ) => {
      if (!session) {
        setInventory(null)
        setInventoryCurrent(false)
        setRuntimeTopology(null)
        return
      }
      if (!background) {
        setRuntimeLoading(true)
        setInventoryCurrent(false)
        setInventory((current) =>
          current?.session === session ? current : null,
        )
        setRuntimeTopology((current) =>
          current?.session === session ? current : null,
        )
        setSelection((current) =>
          current?.kind === 'project' ||
          current?.kind === 'orchestrator' ||
          current?.kind === 'yard-orchestrator' ||
          current?.kind === 'coordination-node' ||
          current?.kind === 'automation' ||
          current?.kind === 'profile' ||
          current?.kind === 'worker' ||
          current?.kind === 'assignment' ||
          current?.kind === 'agent-group'
            ? current
            : null,
        )
      }
      try {
        const result = await fetchInventory(session, signal)
        setInventory((current) => reconcileInventorySnapshot(current, result))
        setInventoryCurrent(true)
        setRuntimeError(null)
        try {
          const topology = await fetchRuntimeTopology(session, signal)
          setRuntimeTopology(topology)
          await Promise.all([
            loadWorkers(signal),
            loadYardOrchestrator(signal),
          ])
        } catch (caught) {
          if (caught instanceof DOMException && caught.name === 'AbortError') {
            return
          }
          setRuntimeError(
            caught instanceof Error
              ? caught.message
              : 'Runtime projection refresh failed',
          )
        }
      } catch (caught) {
        if (caught instanceof DOMException && caught.name === 'AbortError') {
          return
        }
        setInventoryCurrent(false)
        setRuntimeError(
          caught instanceof Error ? caught.message : 'Inventory request failed',
        )
      } finally {
        if (!background && !signal?.aborted) setRuntimeLoading(false)
      }
    },
    [loadWorkers, loadYardOrchestrator],
  )

  const writeProjectTransferContext = useCallback(
    (projectId: string, entry: ProjectTransferContextEntry) => {
      const next = {
        ...projectTransferContextsRef.current,
        [projectId]: entry,
      }
      projectTransferContextsRef.current = next
      setProjectTransferContexts(next)
    },
    [],
  )

  const clearProjectTransferContext = useCallback((projectId: string) => {
    const nextGeneration =
      (projectTransferGenerations.current.get(projectId) ?? 0) + 1
    projectTransferGenerations.current.set(projectId, nextGeneration)
    const next = { ...projectTransferContextsRef.current }
    delete next[projectId]
    projectTransferContextsRef.current = next
    setProjectTransferContexts(next)
  }, [])

  const refreshProjectTransferContext = useCallback(
    async (
      target: {
        adapter: string
        projectId: string
        session: string
      },
      signal?: AbortSignal,
    ): Promise<ProjectTransferSnapshot | null> => {
      const { projectId } = target
      const generation =
        (projectTransferGenerations.current.get(projectId) ?? 0) + 1
      projectTransferGenerations.current.set(projectId, generation)
      const current =
        projectTransferContextsRef.current[projectId] ??
        emptyProjectTransferContext()
      writeProjectTransferContext(
        projectId,
        beginProjectTransferRefresh(current, generation),
      )

      try {
        let adapter = target.adapter
        let session = target.session
        let snapshot: ProjectTransferSnapshot | null = null
        for (let attempt = 0; attempt < 2; attempt += 1) {
          if (adapter !== 'herdr') {
            throw new Error(
              `Runtime adapter ${adapter} does not expose Herdr inventory`,
            )
          }
          const transferInventory = await fetchInventory(session, signal)
          const [workerResult, refreshedProject] = await Promise.all([
            fetchWorkers(signal),
            fetchProject(projectId, signal),
          ])
          if (
            refreshedProject.runtime.adapter === adapter &&
            refreshedProject.runtime.session === session
          ) {
            snapshot = {
              candidates: workerResult.workers,
              inventory: transferInventory,
              project: refreshedProject,
            }
            break
          }
          adapter = refreshedProject.runtime.adapter
          session = refreshedProject.runtime.session
        }
        if (!snapshot) {
          throw new Error(
            'Project runtime changed during transfer refresh. Retry the transfer.',
          )
        }
        if (
          projectTransferGenerations.current.get(projectId) !== generation
        ) {
          return null
        }
        const active =
          projectTransferContextsRef.current[projectId] ??
          emptyProjectTransferContext()
        const resolved = resolveProjectTransferRefresh(
          active,
          generation,
          snapshot,
        )
        writeProjectTransferContext(projectId, resolved.entry)
        return resolved.snapshot
      } catch (caught) {
        if (
          projectTransferGenerations.current.get(projectId) !== generation
        ) {
          return null
        }
        const active =
          projectTransferContextsRef.current[projectId] ??
          emptyProjectTransferContext()
        const aborted =
          caught instanceof DOMException && caught.name === 'AbortError'
        writeProjectTransferContext(
          projectId,
          failProjectTransferRefresh(
            active,
            generation,
            aborted
              ? null
              : caught instanceof Error
                ? caught.message
                : 'Project transfer context request failed',
          ),
        )
        return null
      }
    },
    [writeProjectTransferContext],
  )

  useEffect(() => {
    const controller = new AbortController()
    setRuntimeLoading(true)
    loadSessions(controller.signal)
      .then((availableSessions) => {
        if (!availableSessions.some((session) => session.running)) {
          setRuntimeLoading(false)
        }
      })
      .catch((caught: unknown) => {
        if (caught instanceof DOMException && caught.name === 'AbortError') {
          return
        }
        setRuntimeError(
          caught instanceof Error ? caught.message : 'Session discovery failed',
        )
        setRuntimeLoading(false)
      })
    return () => controller.abort()
  }, [loadSessions])

  useEffect(() => {
    const controller = new AbortController()
    const interval = window.setInterval(() => {
      if (document.visibilityState === 'visible') {
        void loadSessions(controller.signal).catch(() => undefined)
      }
    }, 5_000)
    return () => {
      window.clearInterval(interval)
      controller.abort()
    }
  }, [loadSessions])

  useEffect(() => {
    const controller = new AbortController()
    setProjectLoading(true)
    Promise.all([
      loadProjects(controller.signal),
      loadYardOrchestrator(controller.signal),
      loadCoordination(controller.signal),
      loadAutomations(controller.signal),
      loadTokenSpendSettings(controller.signal),
      loadOrchestratorWorkflowProfile(controller.signal),
      loadProfiles(controller.signal),
      loadWorkers(controller.signal),
    ])
      .catch((caught: unknown) => {
        if (caught instanceof DOMException && caught.name === 'AbortError') {
          return
        }
        setActionError(
          caught instanceof Error ? caught.message : 'Project loading failed',
        )
      })
      .finally(() => {
        if (!controller.signal.aborted) setProjectLoading(false)
      })
    return () => controller.abort()
  }, [
    loadCoordination,
    loadAutomations,
    loadOrchestratorWorkflowProfile,
    loadTokenSpendSettings,
    loadProfiles,
    loadProjects,
    loadWorkers,
    loadYardOrchestrator,
  ])

  useEffect(() => {
    const controller = new AbortController()
    let inFlight = false

    const refreshInventory = async (background: boolean) => {
      if (inFlight) return
      inFlight = true
      try {
        await loadInventory(selectedSession, controller.signal, background)
      } finally {
        inFlight = false
      }
    }

    void refreshInventory(false)
    const refreshWhenVisible = () => {
      if (document.visibilityState === 'visible') {
        void refreshInventory(true)
      }
    }
    const interval = window.setInterval(() => {
      if (document.visibilityState === 'visible') {
        void refreshInventory(true)
      }
    }, INVENTORY_REFRESH_INTERVAL_MS)
    document.addEventListener('visibilitychange', refreshWhenVisible)
    return () => {
      window.clearInterval(interval)
      document.removeEventListener('visibilitychange', refreshWhenVisible)
      controller.abort()
    }
  }, [loadInventory, selectedSession])

  const activeProjectTransferProjectId =
    projectOrchestratorTransfer?.projectId ??
    (selection?.kind === 'orchestrator' ? selection.projectId : null)
  const activeProjectTransferProject = activeProjectTransferProjectId
    ? projects.find(
        (candidate) => candidate.id === activeProjectTransferProjectId,
      )
    : undefined
  const activeProjectTransferAdapter =
    activeProjectTransferProject?.runtime.adapter
  const activeProjectTransferSession =
    activeProjectTransferProject?.runtime.session
  const activeProjectTransferTarget = useMemo(
    () =>
      activeProjectTransferProjectId &&
      activeProjectTransferAdapter &&
      activeProjectTransferSession
      ? {
          adapter: activeProjectTransferAdapter,
          projectId: activeProjectTransferProjectId,
          session: activeProjectTransferSession,
        }
      : null,
    [
      activeProjectTransferAdapter,
      activeProjectTransferProjectId,
      activeProjectTransferSession,
    ],
  )

  useEffect(() => {
    if (!activeProjectTransferTarget) return
    const controller = new AbortController()
    let inFlight = false
    const target = activeProjectTransferTarget

    const refreshTransferContext = async () => {
      if (inFlight || projectOrchestratorTransferBusyRef.current) return
      inFlight = true
      try {
        await refreshProjectTransferContext(target, controller.signal)
      } finally {
        inFlight = false
      }
    }

    void refreshTransferContext()
    const refreshWhenVisible = () => {
      if (document.visibilityState === 'visible') {
        void refreshTransferContext()
      }
    }
    const interval = window.setInterval(() => {
      if (document.visibilityState === 'visible') {
        void refreshTransferContext()
      }
    }, INVENTORY_REFRESH_INTERVAL_MS)
    document.addEventListener('visibilitychange', refreshWhenVisible)
    return () => {
      window.clearInterval(interval)
      document.removeEventListener('visibilitychange', refreshWhenVisible)
      controller.abort()
      clearProjectTransferContext(target.projectId)
    }
  }, [
    activeProjectTransferTarget,
    clearProjectTransferContext,
    refreshProjectTransferContext,
  ])

  useEffect(() => {
    const controller = new AbortController()
    let inFlight = false

    const refreshAssignments = async () => {
      if (inFlight || projectsRef.current.length === 0) return
      inFlight = true
      try {
        await loadAssignments(
          projectsRef.current.map((project) => project.id),
          controller.signal,
        )
      } catch (caught) {
        if (!(caught instanceof DOMException && caught.name === 'AbortError')) {
          // Assignment refresh is independent from runtime health. Keep the
          // last durable projection and retry on the next bounded interval.
        }
      } finally {
        inFlight = false
      }
    }

    const refreshWhenVisible = () => {
      if (document.visibilityState === 'visible') {
        void refreshAssignments()
      }
    }
    const interval = window.setInterval(() => {
      if (document.visibilityState === 'visible') {
        void refreshAssignments()
      }
    }, ASSIGNMENT_REFRESH_INTERVAL_MS)
    document.addEventListener('visibilitychange', refreshWhenVisible)
    return () => {
      window.clearInterval(interval)
      document.removeEventListener('visibilitychange', refreshWhenVisible)
      controller.abort()
    }
  }, [loadAssignments])

  const projectStatusTargets = useMemo(
    () =>
      projects.flatMap((project) =>
        project.orchestrator.runtime
          ? [
              {
                projectId: project.id,
                workerId: project.orchestrator.id,
              },
            ]
          : [],
      ),
    [projects],
  )

  useEffect(() => {
    const controller = new AbortController()
    let inFlight = false

    const refreshProjectStatuses = async () => {
      if (inFlight) return
      inFlight = true
      const results = await Promise.allSettled(
        projectStatusTargets.map(async (target) => ({
          output: await fetchOrchestratorStatusOutput(
            target.projectId,
            controller.signal,
          ),
          target,
        })),
      )
      if (!controller.signal.aborted) {
        setProjectStatusReports((current) => {
          const activeProjectIds = new Set(
            projectStatusTargets.map((target) => target.projectId),
          )
          const next = Object.fromEntries(
            Object.entries(current).filter(([projectId]) =>
              activeProjectIds.has(projectId),
            ),
          )
          results.forEach((result, index) => {
            const target = projectStatusTargets[index]
            if (!target) return
            if (result.status === 'rejected') {
              delete next[target.projectId]
              return
            }
            const { output } = result.value
            const report = parseStatusReport(output.status_report)
            // Reject stale terminal JSON after a newer route. Otherwise an
            // old report could stop the map's active communication signal.
            if (
              report &&
              output.project_id === target.projectId &&
              output.worker_id === target.workerId &&
              statusReportMatchesLatestRoute(
                report,
                target.projectId,
                yardOrchestratorRoutes,
              )
            ) {
              next[target.projectId] = report
            } else {
              delete next[target.projectId]
            }
          })
          return next
        })
      }
      inFlight = false
    }

    void refreshProjectStatuses()
    const refreshWhenVisible = () => {
      if (document.visibilityState === 'visible') {
        void refreshProjectStatuses()
      }
    }
    const interval = window.setInterval(() => {
      if (document.visibilityState === 'visible') {
        void refreshProjectStatuses()
      }
    }, PROJECT_STATUS_REFRESH_INTERVAL_MS)
    document.addEventListener('visibilitychange', refreshWhenVisible)
    return () => {
      window.clearInterval(interval)
      document.removeEventListener('visibilitychange', refreshWhenVisible)
      controller.abort()
    }
  }, [projectStatusTargets, yardOrchestratorRoutes])

  const visibleCandidates = useMemo(
    () => workerCandidates.filter((candidate) => matchesFilter(candidate, filter)),
    [filter, workerCandidates],
  )
  const visibleWorkers = inventory?.workers ?? []
  const allocationPayloadByRuntimeId = useMemo(
    () =>
      Object.fromEntries(
        (inventory?.workers ?? []).flatMap((worker) => {
          const candidate = findCandidateForObservedWorker(
            workerCandidates,
            inventory,
            worker,
          )
          return candidate && canAllocateCandidate(candidate)
            ? [
                [
                  worker.runtime_id,
                  { kind: 'worker' as const, id: candidate.worker.id },
                ],
              ]
            : []
        }),
      ),
    [inventory, workerCandidates],
  )
  const boundWorkspaceIds = useMemo(
    () =>
      new Set(
        projects
          .filter(
            (project) =>
              project.runtime.adapter === 'herdr' &&
              project.runtime.session === selectedSession,
          )
          .map((project) => project.runtime.workspace_id),
      ),
    [projects, selectedSession],
  )
  const availableWorkspaces = useMemo(
    () => {
      const managedWorkspaceIds = new Set(
        runtimeTopology?.managed_workspaces.map(
          (workspace) => workspace.workspace_id,
        ) ?? [],
      )
      return (
        inventory?.workspaces.filter(
          (workspace) =>
            !boundWorkspaceIds.has(workspace.runtime_id) &&
            !managedWorkspaceIds.has(workspace.runtime_id),
        ) ?? []
      )
    },
    [boundWorkspaceIds, inventory, runtimeTopology],
  )

  const selectedObservedWorker =
    selection?.kind === 'observed-worker'
      ? inventory?.workers.find(
          (worker) => worker.runtime_id === selection.id,
        )
      : undefined
  const selectedProviderChild =
    selection?.kind === 'provider-child'
      ? inventory?.child_agents?.find(
          (agent) => agent.runtime_id === selection.id,
        )
      : undefined
  const selectedWorkerCandidate =
    selection?.kind === 'worker'
      ? workerCandidates.find(
          (candidate) => candidate.worker.id === selection.id,
        )
      : selectedObservedWorker
        ? findCandidateForObservedWorker(
            workerCandidates,
            inventory,
            selectedObservedWorker,
          )
        : undefined
  const selectedWorkspace =
    selection?.kind === 'workspace'
      ? inventory?.workspaces.find(
          (workspace) => workspace.runtime_id === selection.id,
        )
      : undefined
  const selectedProject =
    selection?.kind === 'project'
      ? projects.find((project) => project.id === selection.id)
      : undefined
  const projectArchiveActiveAssignmentCount = projectArchiveProposal
    ? assignments.filter(
        (assignment) =>
          assignment.project_id === projectArchiveProposal.project.id &&
          ['allocating', 'active', 'handing_off'].includes(
            assignment.lifecycle,
          ),
      ).length
    : 0
  const projectDeleteActiveAssignmentCount = projectDeleteProposal
    ? assignments.filter(
        (assignment) =>
          assignment.project_id === projectDeleteProposal.project.id &&
          ['allocating', 'active', 'handing_off'].includes(
            assignment.lifecycle,
          ),
      ).length
    : 0
  const selectedProjectOrchestrator =
    selection?.kind === 'orchestrator'
      ? projects.find((project) => project.id === selection.projectId)
      : undefined
  const selectedProjectTransferContext = selectedProjectOrchestrator
    ? projectTransferContexts[selectedProjectOrchestrator.id]
    : undefined
  const selectedProjectTransferSnapshot =
    selectedProjectTransferContext?.snapshot
  const selectedProjectOrchestratorContextProject =
    selectedProjectTransferSnapshot?.project ??
    selectedProjectOrchestrator
  const selectedProjectOrchestratorEligibility = useMemo(
    () =>
      selectedProjectTransferSnapshot &&
      !selectedProjectTransferContext?.loading &&
      !selectedProjectTransferContext.error
        ? projectOrchestratorEligibility(
            selectedProjectTransferSnapshot.inventory,
            selectedProjectTransferSnapshot.project,
            selectedProjectTransferSnapshot.candidates,
          )
        : {
            candidates: [],
            reason: 'inventory_unavailable' as const,
          },
    [
      selectedProjectTransferContext?.error,
      selectedProjectTransferContext?.loading,
      selectedProjectTransferSnapshot,
    ],
  )
  const projectOrchestratorTransferContext = projectOrchestratorTransfer
    ? projectTransferContexts[projectOrchestratorTransfer.projectId]
    : undefined
  const projectOrchestratorTransferSnapshot =
    projectOrchestratorTransferContext?.snapshot
  const projectOrchestratorTransferProject =
    projectOrchestratorTransferSnapshot?.project ??
    (projectOrchestratorTransfer
      ? projects.find(
          (project) => project.id === projectOrchestratorTransfer.projectId,
        )
      : undefined)
  const projectOrchestratorTransferEligibility = useMemo(
    () =>
      projectOrchestratorTransferSnapshot &&
      !projectOrchestratorTransferContext?.loading &&
      !projectOrchestratorTransferContext.error
        ? projectOrchestratorEligibility(
            projectOrchestratorTransferSnapshot.inventory,
            projectOrchestratorTransferSnapshot.project,
            projectOrchestratorTransferSnapshot.candidates,
          )
        : {
            candidates: [],
            reason: 'inventory_unavailable' as const,
          },
    [
      projectOrchestratorTransferContext?.error,
      projectOrchestratorTransferContext?.loading,
      projectOrchestratorTransferSnapshot,
    ],
  )
  const selectedYardOrchestrator =
    selection?.kind === 'yard-orchestrator'
      ? yardOrchestrator ?? undefined
      : undefined
  const selectedCoordinationNode =
    selection?.kind === 'coordination-node'
      ? coordinationNodes.find((node) => node.id === selection.id)
      : undefined
  const selectedAutomation =
    selection?.kind === 'automation'
      ? automations.find((automation) => automation.id === selection.id)
      : undefined
  const selectedProfile =
    selection?.kind === 'profile'
      ? profiles.find((profile) => profile.id === selection.id)
      : undefined
  const selectedAssignment =
    selection?.kind === 'assignment'
      ? assignments.find((assignment) => assignment.id === selection.id)
      : undefined
  const selectedAssignmentCandidate = selectedAssignment
    ? workerCandidates.find(
        (candidate) =>
          candidate.worker.id === selectedAssignment.worker.id,
      )
    : undefined
  const selectedAgentGroup = useMemo<AgentGroupTarget[]>(() => {
    if (selection?.kind !== 'agent-group') return []
    return selection.targets.flatMap((target): AgentGroupTarget[] => {
      if (target.kind === 'assignment') {
        const assignment = assignments.find(
          (candidate) =>
            candidate.id === target.id &&
            candidate.lifecycle === 'active',
        )
        if (!assignment) return []
        const project = projects.find(
          (candidate) => candidate.id === assignment.project_id,
        )
        const capabilities = resolveRuntimeCapabilities(
          inventoryCurrent,
          assignment.worker.runtime,
          inventory,
        )
        return [
          {
            assignment,
            kind: 'assignment',
            label: assignment.profile_name,
            projectName: project?.name ?? 'Yard project',
            status: runtimeCapabilityStatus(
              assignment.worker.runtime,
              capabilities,
            ),
          },
        ]
      }
      const project = projects.find(
        (candidate) => candidate.id === target.projectId,
      )
      if (!project) return []
      const capabilities = resolveRuntimeCapabilities(
        inventoryCurrent,
        project.orchestrator.runtime,
        inventory,
      )
      return [
        {
          kind: 'orchestrator',
          label: `${project.name} orchestrator`,
          project,
          status: runtimeCapabilityStatus(
            project.orchestrator.runtime,
            capabilities,
          ),
        },
      ]
    })
  }, [assignments, inventory, inventoryCurrent, projects, selection])
  const selectedCandidateCompletion = selectedWorkerCandidate
    ? assignments
        .filter(
          (assignment) =>
            assignment.worker.id === selectedWorkerCandidate.worker.id &&
            assignment.lifecycle === 'completed',
        )
        .sort(
          (left, right) =>
            right.updated_at_unix_ms - left.updated_at_unix_ms,
        )[0]
    : undefined
  const selectedCandidateActiveAssignment = selectedWorkerCandidate
    ? assignments
        .filter(
          (assignment) =>
            assignment.worker.id === selectedWorkerCandidate.worker.id &&
            assignment.lifecycle === 'active',
        )
        .sort(
          (left, right) =>
            right.updated_at_unix_ms - left.updated_at_unix_ms,
        )[0]
    : undefined

  useEffect(() => {
    if (selection?.kind !== 'yard-orchestrator') return
    const controller = new AbortController()
    let inFlight = false
    const refreshPortfolio = async () => {
      if (inFlight) return
      inFlight = true
      try {
        await Promise.all([
          loadProjects(controller.signal),
          loadCoordination(controller.signal),
        ])
      } catch (caught) {
        if (!(caught instanceof DOMException && caught.name === 'AbortError')) {
          setActionError(
            caught instanceof Error
              ? caught.message
              : 'Portfolio update failed',
          )
        }
      } finally {
        inFlight = false
      }
    }
    const interval = window.setInterval(() => {
      if (document.visibilityState === 'visible') void refreshPortfolio()
    }, 10_000)
    return () => {
      window.clearInterval(interval)
      controller.abort()
    }
  }, [loadCoordination, loadProjects, selection?.kind])

  useEffect(() => {
    if (selection?.kind !== 'coordination-node') return
    const controller = new AbortController()
    let inFlight = false
    const refreshNode = async () => {
      if (inFlight) return
      inFlight = true
      try {
        await loadCoordination(controller.signal)
      } catch (caught) {
        if (!(caught instanceof DOMException && caught.name === 'AbortError')) {
          setActionError(
            caught instanceof Error
              ? caught.message
              : 'Coordination node refresh failed',
          )
        }
      } finally {
        inFlight = false
      }
    }
    const interval = window.setInterval(() => {
      if (document.visibilityState === 'visible') void refreshNode()
    }, 5_000)
    return () => {
      window.clearInterval(interval)
      controller.abort()
    }
  }, [loadCoordination, selection?.kind])

  useEffect(() => {
    if (selection?.kind !== 'automation') return
    const controller = new AbortController()
    let inFlight = false
    const refreshAutomation = async () => {
      if (inFlight) return
      inFlight = true
      setAutomationHistoryLoading(true)
      try {
        const [automation, runs] = await Promise.all([
          fetchAutomation(selection.id, controller.signal),
          loadAutomationRuns(selection.id, controller.signal),
        ])
        setAutomations((current) =>
          current.map((candidate) =>
            candidate.id === automation.id ? automation : candidate,
          ),
        )
        setAutomationRuns((current) => ({
          ...current,
          [selection.id]: runs,
        }))
        setAutomationError(null)
      } catch (caught) {
        if (!(caught instanceof DOMException && caught.name === 'AbortError')) {
          setAutomationError(
            caught instanceof Error
              ? caught.message
              : 'Automation refresh failed',
          )
        }
      } finally {
        if (!controller.signal.aborted) setAutomationHistoryLoading(false)
        inFlight = false
      }
    }
    void refreshAutomation()
    const interval = window.setInterval(() => {
      if (document.visibilityState === 'visible') void refreshAutomation()
    }, 5_000)
    return () => {
      window.clearInterval(interval)
      controller.abort()
    }
  }, [loadAutomationRuns, selection])
  const selectedWorkspaceWorkers = selectedWorkspace
    ? (inventory?.workers.filter(
        (worker) => worker.workspace_id === selectedWorkspace.runtime_id,
      ) ?? [])
    : []
  const agentWorkspaceTargets = useMemo<AgentWorkspaceTarget[]>(() => {
    const targets: AgentWorkspaceTarget[] = []
    const capabilitiesForRuntime = (runtime: WorkerRuntimeBinding) =>
      resolveRuntimeCapabilities(inventoryCurrent, runtime, inventory)
    const projectNames = new Map(
      projects.map((project) => [project.id, project.name]),
    )
    const yardWorker = yardOrchestrator?.worker
    const yardRuntime = yardWorker?.runtime
    if (yardOrchestrator && yardWorker && yardRuntime) {
      targets.push({
        ...agentTargetRuntimeMetadata(
          yardRuntime,
          capabilitiesForRuntime(yardRuntime),
        ),
        contextLabel: 'Yard portfolio',
        key: 'yard-orchestrator',
        label: 'Superintendent',
        role: 'superintendent',
        roleLabel: 'Superintendent',
        session: yardRuntime.session,
        target: { kind: 'yard-orchestrator', orchestrator: yardOrchestrator },
        terminalId: yardRuntime.terminal_id,
        terminalLeaseKey: terminalLeaseKey(
          { kind: 'yard-orchestrator', orchestrator: yardOrchestrator },
          yardRuntime,
        ),
        terminalTarget: {
          kind: 'yard-orchestrator',
          workerId: yardWorker.id,
        },
        workspaceId: yardRuntime.workspace_id,
      })
    }

    coordinationNodes.forEach((node) => {
      const worker = node.worker
      const runtime = worker?.runtime
      if (!worker || !runtime || node.kind !== 'workstream') return
      const attachedProjects = node.attached_project_ids
        .map((projectId) => projectNames.get(projectId))
        .filter((name): name is string => Boolean(name))
      targets.push({
        ...agentTargetRuntimeMetadata(
          runtime,
          capabilitiesForRuntime(runtime),
          node.cwd ?? node.folder_path,
        ),
        contextLabel:
          attachedProjects.join(', ') || 'Yard workstream',
        key: `coordination-node:${node.id}`,
        label: node.name,
        role: 'workstream',
        roleLabel: 'Workstream',
        session: runtime.session,
        target: { kind: 'coordination-node', node },
        terminalId: runtime.terminal_id,
        terminalLeaseKey: terminalLeaseKey(
          { kind: 'coordination-node', node },
          runtime,
        ),
        terminalTarget: {
          kind: 'coordination-node',
          nodeId: node.id,
          workerId: worker.id,
        },
        workspaceId: runtime.workspace_id,
      })
    })

    projects.forEach((project) => {
      const runtime = project.orchestrator.runtime
      if (!runtime) return
      targets.push({
        ...agentTargetRuntimeMetadata(
          runtime,
          capabilitiesForRuntime(runtime),
        ),
        contextLabel: project.name,
        key: `orchestrator:${project.id}`,
        label: `${project.name} orchestrator`,
        role: 'orchestrator',
        roleLabel: 'Orchestrator',
        session: runtime.session,
        target: { kind: 'orchestrator', project },
        terminalId: runtime.terminal_id,
        terminalLeaseKey: terminalLeaseKey(
          { kind: 'orchestrator', project },
          runtime,
        ),
        terminalTarget: {
          kind: 'orchestrator',
          projectId: project.id,
          workerId: project.orchestrator.id,
        },
        workspaceId: runtime.workspace_id,
      })
    })

    assignments.forEach((assignment) => {
      const runtime = assignment.worker.runtime
      if (!runtime || assignment.lifecycle !== 'active') return
      targets.push({
        ...agentTargetRuntimeMetadata(
          runtime,
          capabilitiesForRuntime(runtime),
        ),
        contextLabel:
          projectNames.get(assignment.project_id) ?? 'Yard project',
        key: `assignment:${assignment.id}`,
        label: assignment.profile_name,
        role: 'worker',
        roleLabel: assignment.role,
        session: runtime.session,
        target: { kind: 'assignment', assignment },
        terminalId: runtime.terminal_id,
        terminalLeaseKey: terminalLeaseKey(
          { kind: 'assignment', assignment },
          runtime,
        ),
        terminalTarget: {
          kind: 'assignment',
          assignmentId: assignment.id,
          projectId: assignment.project_id,
        },
        workspaceId: runtime.workspace_id,
      })
    })
    return targets
  }, [
    assignments,
    coordinationNodes,
    inventory,
    inventoryCurrent,
    projects,
    yardOrchestrator,
  ])
  const currentAgentWorkspaceTarget = agentWorkspaceTarget
    ? agentWorkspaceTargets.find(
        (target) => target.key === agentWorkspaceTarget.key,
      )
    : null
  const activeAgentWorkspaceTarget = currentAgentWorkspaceTarget
    ? {
        ...currentAgentWorkspaceTarget,
        returnFocus: agentWorkspaceTarget?.returnFocus,
      }
    : agentWorkspaceTargets[0] ?? null
  const openAgentChat = useCallback((target: AgentWorkspaceTarget) => {
    if (!target.chatAvailable) return
    setAgentWorkspaceTarget(target)
    setAgentWorkspaceMode('chat')
  }, [])
  const openAgentTerminal = useCallback(
    (target: AgentWorkspaceTarget) => {
      if (!target.interactive) return
      setAgentWorkspaceTarget(target)
      setTerminalPresentation(DEFAULT_TERMINAL_PRESENTATION)
      setAgentWorkspaceMode('terminal')
    },
    [],
  )
  const agentWorkspaceContext = useMemo(
    () => ({
      openChat: openAgentChat,
      openTerminal: openAgentTerminal,
    }),
    [openAgentChat, openAgentTerminal],
  )

  const refresh = useCallback(async () => {
    setActionError(null)
    try {
      const [refreshedSessions] = await Promise.all([
        loadSessions(),
        loadProjects(),
        loadCoordination(),
        loadAutomations(),
        loadTokenSpendSettings(),
        loadOrchestratorWorkflowProfile(),
        loadProfiles(),
        loadWorkers(),
        loadYardOrchestrator(),
      ])
      const refreshedSession =
        refreshedSessions.find(
          (session) => session.name === selectedSession && session.running,
        )?.name ??
        refreshedSessions.find(
          (session) => session.is_default && session.running,
        )?.name ??
        refreshedSessions.find((session) => session.running)?.name ??
        ''
      await loadInventory(refreshedSession)
    } catch (caught) {
      setActionError(
        caught instanceof Error ? caught.message : 'Refresh failed',
      )
      setRuntimeLoading(false)
      setProjectLoading(false)
    }
  }, [
    loadInventory,
    loadAutomations,
    loadCoordination,
    loadOrchestratorWorkflowProfile,
    loadProfiles,
    loadProjects,
    loadSessions,
    loadWorkers,
    loadYardOrchestrator,
    loadTokenSpendSettings,
    selectedSession,
  ])

  const saveTokenSpendSettings = useCallback(
    async ({
      projectOrchestrators,
      scheduled,
      superintendent,
    }: {
      projectOrchestrators: boolean
      scheduled: boolean
      superintendent: boolean
    }) => {
      if (!tokenSpendSettings) return
      setTokenSpendSettingsBusy(true)
      setTokenSpendSettingsError(null)
      try {
        const updated = await updateTokenSpendSettings({
          actor: 'local-user',
          expected_version: tokenSpendSettings.version,
          superintendent_auto_requests_project_summaries: superintendent,
          project_orchestrators_auto_request_worker_summaries:
            projectOrchestrators,
          scheduled_automatic_summaries: scheduled,
        })
        setTokenSpendSettings(updated)
        setTokenSpendSettingsOpen(false)
      } catch (caught) {
        setTokenSpendSettingsError(
          caught instanceof Error
            ? caught.message
            : 'Automatic settings update failed',
        )
        await loadTokenSpendSettings().catch(() => undefined)
      } finally {
        setTokenSpendSettingsBusy(false)
      }
    },
    [loadTokenSpendSettings, tokenSpendSettings],
  )

  const saveOrchestratorWorkflow = useCallback(
    async ({
      instructionsMarkdown,
      monitorIntervalMs,
    }: {
      instructionsMarkdown: string
      monitorIntervalMs: number
    }) => {
      if (!orchestratorWorkflowProfile) return
      setOrchestratorWorkflowBusy(true)
      setOrchestratorWorkflowError(null)
      try {
        const updated = await updateOrchestratorWorkflowProfile({
          actor: 'local-user',
          expected_version: orchestratorWorkflowProfile.version,
          instructions_markdown: instructionsMarkdown,
          monitor_interval_ms: String(monitorIntervalMs),
        })
        setOrchestratorWorkflowProfile(updated)
        setOrchestratorWorkflowOpen(false)
      } catch (caught) {
        setOrchestratorWorkflowError(
          caught instanceof Error
            ? caught.message
            : 'Orchestrator workflow update failed',
        )
        await loadOrchestratorWorkflowProfile().catch(() => undefined)
      } finally {
        setOrchestratorWorkflowBusy(false)
      }
    },
    [loadOrchestratorWorkflowProfile, orchestratorWorkflowProfile],
  )

  const resetOrchestratorWorkflow = useCallback(async () => {
    if (!orchestratorWorkflowProfile) return
    setOrchestratorWorkflowBusy(true)
    setOrchestratorWorkflowError(null)
    try {
      const reset = await resetOrchestratorWorkflowProfile({
        actor: 'local-user',
        expected_version: orchestratorWorkflowProfile.version,
      })
      setOrchestratorWorkflowProfile(reset)
    } catch (caught) {
      setOrchestratorWorkflowError(
        caught instanceof Error
          ? caught.message
          : 'Orchestrator workflow reset failed',
      )
      await loadOrchestratorWorkflowProfile().catch(() => undefined)
    } finally {
      setOrchestratorWorkflowBusy(false)
    }
  }, [loadOrchestratorWorkflowProfile, orchestratorWorkflowProfile])

  const proposeProjectOrchestratorTransfer = useCallback(
    (project: Project, returnFocus: HTMLElement) => {
      setActionError(null)
      setActionNotice(null)
      setProjectOrchestratorTransferError(null)
      setProjectOrchestratorTransfer({
        commandId: crypto.randomUUID(),
        projectId: project.id,
        returnFocus,
      })
      void refreshProjectTransferContext({
        adapter: project.runtime.adapter,
        projectId: project.id,
        session: project.runtime.session,
      })
    },
    [refreshProjectTransferContext],
  )

  const transferProjectOrchestrator = useCallback(
    async (workerId: string) => {
      if (!projectOrchestratorTransfer) return
      const projectId = projectOrchestratorTransfer.projectId
      const currentProject =
        projectOrchestratorTransferSnapshot?.project ??
        projects.find((candidate) => candidate.id === projectId)
      if (!currentProject) return

      projectOrchestratorTransferBusyRef.current = true
      setProjectOrchestratorTransferBusy(true)
      setProjectOrchestratorTransferError(null)
      setActionError(null)
      try {
        const snapshot = await refreshProjectTransferContext({
          adapter: currentProject.runtime.adapter,
          projectId,
          session: currentProject.runtime.session,
        })
        const project = snapshot?.project
        const candidate = snapshot?.candidates.find(
          ({ worker }) => worker.id === workerId,
        )
        const workerRuntime = candidate?.worker.runtime
        const orchestratorRuntime = project?.orchestrator.runtime
        if (
          !snapshot ||
          !project ||
          !candidate ||
          !workerRuntime ||
          !orchestratorRuntime ||
          !projectOrchestratorEligibility(
            snapshot.inventory,
            project,
            [candidate],
          ).candidates.length
        ) {
          setProjectOrchestratorTransferError(
            'The selected worker is no longer eligible. Review the refreshed runtime state and retry.',
          )
          return
        }

        const result = await changeProjectOrchestrator(project.id, {
          command_id: projectOrchestratorTransfer.commandId,
          actor: 'local-user',
          worker_id: candidate.worker.id,
          expected_worker_version: candidate.worker.version,
          expected_worker_runtime: workerRuntime,
          expected_project_version: project.version,
          expected_orchestrator_worker_id: project.orchestrator.id,
          expected_orchestrator_worker_version: project.orchestrator.version,
          expected_orchestrator_runtime: orchestratorRuntime,
        })
        setProjects((current) =>
          current.map((item) =>
            item.id === result.project.id ? result.project : item,
          ),
        )
        try {
          await Promise.all([
            loadProjects(),
            loadWorkers(),
            loadInventory(selectedSession),
          ])
          await refreshProjectTransferContext({
            adapter: result.project.runtime.adapter,
            projectId,
            session: result.project.runtime.session,
          })
        } catch (caught) {
          setActionError(
            caught instanceof Error
              ? `Orchestrator changed, but refresh failed: ${caught.message}`
              : 'Orchestrator changed, but refresh failed',
          )
        }
        setProjectOrchestratorTransfer(null)
      } catch (caught) {
        let reconciledProject: Project | undefined
        try {
          await Promise.all([
            loadProjects(),
            loadWorkers(),
            loadInventory(selectedSession),
          ])
          const reconciledSnapshot = await refreshProjectTransferContext({
            adapter: currentProject.runtime.adapter,
            projectId,
            session: currentProject.runtime.session,
          })
          reconciledProject = reconciledSnapshot?.project
          const loadedProjects = await fetchProjects()
          await loadAssignments(
            loadedProjects.projects.map((item) => item.id),
          )
        } catch {
          // Keep the original command ID when the outcome cannot be reconciled.
        }

        if (reconciledProject?.orchestrator.id === workerId) {
          setProjectOrchestratorTransferError(null)
          setProjectOrchestratorTransfer(null)
          setActionNotice(
            'Orchestrator changed. Yard reconciled the project after the response was unavailable.',
          )
        } else {
          setProjectOrchestratorTransferError(
            caught instanceof Error
              ? caught.message
              : 'Project orchestrator change failed',
          )
          if (
            caught instanceof YardApiError &&
            (caught.code === 'project_orchestrator_transfer_conflict' ||
              caught.code === 'project_orchestrator_identity_changed')
          ) {
            setProjectOrchestratorTransfer((current) =>
              current
                ? { ...current, commandId: crypto.randomUUID() }
                : current,
            )
          }
        }
      } finally {
        projectOrchestratorTransferBusyRef.current = false
        setProjectOrchestratorTransferBusy(false)
      }
    },
    [
      loadInventory,
      loadAssignments,
      loadProjects,
      loadWorkers,
      projectOrchestratorTransfer,
      projectOrchestratorTransferSnapshot,
      projects,
      refreshProjectTransferContext,
      selectedSession,
    ],
  )

  const proposeEndSession = useCallback((candidate: WorkerCandidate) => {
    if (!canEndCandidate(candidate)) return
    setEndSessionError(null)
    setActionNotice(null)
    setEndSessionProposal({
      candidate,
      commandId: crypto.randomUUID(),
    })
  }, [])

  const endSession = useCallback(async () => {
    if (!endSessionProposal) return
    const { candidate, commandId } = endSessionProposal
    setEndSessionBusy(true)
    setEndSessionError(null)
    setActionError(null)
    try {
      const result = await endWorkerSession(candidate.worker.id, {
        command_id: commandId,
        actor: 'local-user',
        expected_worker_version: candidate.worker.version,
        ...(candidate.worker.runtime
          ? {
              expected_runtime_version:
                candidate.worker.runtime.version,
            }
          : {}),
      })
      setActionNotice(
        result.cleanup_pending
          ? 'Session ended. Verified runtime cleanup is queued.'
          : null,
      )
      await Promise.all([
        loadProjects(),
        loadWorkers(),
        loadInventory(selectedSession),
      ])
      setEndSessionProposal(null)
    } catch (caught) {
      const message =
        caught instanceof Error ? caught.message : 'End session failed'
      const workers = await loadWorkers().catch(() => null)
      const reconciled = workers?.find(
        ({ worker }) => worker.id === candidate.worker.id,
      )
      if (reconciled?.worker.desired_state === 'ended') {
        setActionNotice(
          'Session ended. Runtime cleanup will continue in the background if needed.',
        )
        setEndSessionProposal(null)
      } else {
        setEndSessionError(message)
      }
    } finally {
      setEndSessionBusy(false)
    }
  }, [
    endSessionProposal,
    loadInventory,
    loadProjects,
    loadWorkers,
    selectedSession,
  ])

  const proposeWorkerDelete = useCallback((candidate: WorkerCandidate) => {
    if (!canDeleteCandidate(candidate)) return
    setWorkerDeleteError(null)
    setActionNotice(null)
    setWorkerDeleteProposal({
      candidate,
      deleteCommandId: crypto.randomUUID(),
      endCommandId: crypto.randomUUID(),
    })
  }, [])

  const deleteSelectedWorker = useCallback(async () => {
    if (!workerDeleteProposal) return
    const { candidate, deleteCommandId, endCommandId } =
      workerDeleteProposal
    setWorkerDeleteBusy(true)
    setWorkerDeleteError(null)
    setActionError(null)
    try {
      let expectedWorkerVersion = candidate.worker.version
      if (candidate.worker.desired_state !== 'ended') {
        const ended = await endWorkerSession(candidate.worker.id, {
          command_id: endCommandId,
          actor: 'local-user',
          expected_worker_version: candidate.worker.version,
          ...(candidate.worker.runtime
            ? {
                expected_runtime_version:
                  candidate.worker.runtime.version,
              }
            : {}),
        })
        expectedWorkerVersion = ended.worker.version
      }
      const result = await deleteWorker(candidate.worker.id, {
        command_id: deleteCommandId,
        actor: 'local-user',
        expected_worker_version: expectedWorkerVersion,
      })
      setWorkerCandidates((current) =>
        current.filter(
          ({ worker }) => worker.id !== candidate.worker.id,
        ),
      )
      setSelection((current) =>
        current?.kind === 'worker' &&
        current.id === candidate.worker.id
          ? null
          : current,
      )
      setWorkerDeleteProposal(null)
      setActionNotice(
        result.cleanup_pending
          ? 'Worker deleted from Yard. Runtime cleanup continues in the background.'
          : 'Worker deleted from Yard.',
      )
      await Promise.all([
        loadProjects(),
        loadWorkers(),
        loadInventory(selectedSession),
      ]).catch(() => undefined)
    } catch (caught) {
      const message =
        caught instanceof Error ? caught.message : 'Delete worker failed'
      const workers = await loadWorkers().catch(() => null)
      const reconciled = workers?.find(
        ({ worker }) => worker.id === candidate.worker.id,
      )
      if (!reconciled && workers) {
        setWorkerDeleteProposal(null)
        setSelection(null)
        setActionNotice(
          'Worker deleted from Yard. Runtime cleanup continues in the background if needed.',
        )
      } else {
        if (reconciled) {
          setWorkerDeleteProposal((current) =>
            current ? { ...current, candidate: reconciled } : current,
          )
        }
        setWorkerDeleteError(message)
      }
    } finally {
      setWorkerDeleteBusy(false)
    }
  }, [
    loadInventory,
    loadProjects,
    loadWorkers,
    selectedSession,
    workerDeleteProposal,
  ])

  const proposeProjectArchive = useCallback(
    (project: Project, returnFocus: HTMLButtonElement) => {
      setProjectArchiveError(null)
      setActionNotice(null)
      setProjectArchiveProposal({
        commandId: crypto.randomUUID(),
        project,
        returnFocus,
      })
    },
    [],
  )

  const archiveSelectedProject = useCallback(async () => {
    if (!projectArchiveProposal) return
    const { commandId, project } = projectArchiveProposal
    setProjectArchiveBusy(true)
    setProjectArchiveError(null)
    setActionError(null)

    const reconcileArchivedProject = async (cleanupPending: boolean) => {
      setProjects((current) =>
        current.filter((candidate) => candidate.id !== project.id),
      )
      setProjectRelationships((current) =>
        current.filter(
          (relationship) =>
            relationship.source_project_id !== project.id &&
            relationship.target_project_id !== project.id,
        ),
      )
      setAssignments((current) =>
        current.filter(
          (assignment) => assignment.project_id !== project.id,
        ),
      )
      setProjectStatusReports((current) => {
        const next = { ...current }
        delete next[project.id]
        return next
      })
      clearProjectTransferContext(project.id)
      setSelection(null)
      setProjectArchiveProposal(null)
      setActionNotice(
        cleanupPending
          ? 'Project archived. Verified orchestrator cleanup is queued.'
          : 'Project archived.',
      )
      await Promise.all([
        loadProjects(),
        loadCoordination(),
        loadWorkers(),
        loadInventory(selectedSession),
      ]).catch(() => undefined)
    }

    try {
      const result = await archiveProject(project.id, {
        command_id: commandId,
        actor: 'local-user',
        expected_project_version: project.version,
        expected_orchestrator_worker_id: project.orchestrator.id,
        expected_orchestrator_worker_version:
          project.orchestrator.version,
        expected_orchestrator_runtime_version:
          project.orchestrator.runtime?.version ?? null,
      })
      await reconcileArchivedProject(result.cleanup_pending)
    } catch (caught) {
      const message =
        caught instanceof Error ? caught.message : 'Project archive failed'
      const activeProjects = await fetchProjects().catch(() => null)
      if (
        activeProjects &&
        !activeProjects.projects.some(
          (candidate) => candidate.id === project.id,
        )
      ) {
        await reconcileArchivedProject(true)
      } else {
        setProjectArchiveError(message)
      }
    } finally {
      setProjectArchiveBusy(false)
    }
  }, [
    clearProjectTransferContext,
    loadCoordination,
    loadInventory,
    loadProjects,
    loadWorkers,
    projectArchiveProposal,
    selectedSession,
  ])

  const proposeProjectDelete = useCallback(
    (project: Project, returnFocus: HTMLButtonElement) => {
      setProjectDeleteError(null)
      setActionNotice(null)
      setProjectDeleteProposal({
        archiveCommandId: crypto.randomUUID(),
        deleteCommandId: crypto.randomUUID(),
        project,
        returnFocus,
      })
    },
    [],
  )

  const deleteSelectedProject = useCallback(async () => {
    if (!projectDeleteProposal) return
    const { archiveCommandId, deleteCommandId, project } =
      projectDeleteProposal
    setProjectDeleteBusy(true)
    setProjectDeleteError(null)
    setActionError(null)

    const reconcileDeletedProject = async (cleanupPending: boolean) => {
      setProjects((current) =>
        current.filter((candidate) => candidate.id !== project.id),
      )
      setProjectRelationships((current) =>
        current.filter(
          (relationship) =>
            relationship.source_project_id !== project.id &&
            relationship.target_project_id !== project.id,
        ),
      )
      setAssignments((current) =>
        current.filter(
          (assignment) => assignment.project_id !== project.id,
        ),
      )
      setWorkerCandidates((current) =>
        current.filter(
          ({ worker }) => worker.id !== project.orchestrator.id,
        ),
      )
      setProjectStatusReports((current) => {
        const next = { ...current }
        delete next[project.id]
        return next
      })
      clearProjectTransferContext(project.id)
      setSelection(null)
      setProjectDeleteProposal(null)
      setActionNotice(
        cleanupPending
          ? 'Project deleted from Yard. Runtime cleanup continues in the background.'
          : 'Project deleted from Yard.',
      )
      await Promise.all([
        loadProjects(),
        loadCoordination(),
        loadWorkers(),
        loadInventory(selectedSession),
      ]).catch(() => undefined)
    }

    try {
      await archiveProject(project.id, {
        command_id: archiveCommandId,
        actor: 'local-user',
        expected_project_version: project.version,
        expected_orchestrator_worker_id: project.orchestrator.id,
        expected_orchestrator_worker_version:
          project.orchestrator.version,
        expected_orchestrator_runtime_version:
          project.orchestrator.runtime?.version ?? null,
      })
      const result = await deleteProject(project.id, {
        command_id: deleteCommandId,
        actor: 'local-user',
      })
      await reconcileDeletedProject(result.cleanup_pending)
    } catch (caught) {
      const message =
        caught instanceof Error ? caught.message : 'Delete project failed'
      const [activeProjects, workers] = await Promise.all([
        fetchProjects().catch(() => null),
        loadWorkers().catch(() => null),
      ])
      const projectMissing =
        activeProjects !== null &&
        !activeProjects.projects.some(
          (candidate) => candidate.id === project.id,
        )
      const orchestratorMissing =
        workers !== null &&
        !workers.some(
          ({ worker }) => worker.id === project.orchestrator.id,
        )
      if (projectMissing && orchestratorMissing) {
        await reconcileDeletedProject(true)
      } else {
        setProjectDeleteError(message)
      }
    } finally {
      setProjectDeleteBusy(false)
    }
  }, [
    clearProjectTransferContext,
    loadCoordination,
    loadInventory,
    loadProjects,
    loadWorkers,
    projectDeleteProposal,
    selectedSession,
  ])

  const createWorkspaceProject = useCallback(
    async (
      workspace: WorkspaceObservation,
      details: ProjectCreationDetails,
    ) => {
      const workerCount =
        inventory?.workers.filter(
          (worker) => worker.workspace_id === workspace.runtime_id,
        ).length ?? 0
      setAdoptingWorkspace(workspace.runtime_id)
      setActionError(null)
      try {
        const runtime = {
          adapter: 'herdr',
          session: selectedSession,
          workspace_id: workspace.runtime_id,
        }
        const placement = nextProjectPlacement(
          projects,
          workerCount + (details.mode === 'profile' ? 1 : 0),
        )
        const project =
          details.mode === 'existing'
            ? await createProject({
                name: details.name,
                runtime,
                orchestrator_observed_worker_id: details.orchestratorId,
                placement,
              })
            : (
                await createProjectFromProfile({
                  command_id: details.commandId,
                  actor: 'local-user',
                  name: details.name,
                  runtime,
                  profile_id: details.profileId,
                  expected_profile_version:
                    profiles.find(
                      (profile) => profile.id === details.profileId,
                    )?.version ?? '',
                  orchestrator_objective: details.objective,
                  placement,
                })
              ).project
        setProjects((current) => [
          ...current.filter((candidate) => candidate.id !== project.id),
          project,
        ])
        setSelection({ kind: 'project', id: project.id })
        await Promise.all([
          loadInventory(selectedSession),
          loadWorkers(),
        ])
      } catch (caught) {
        setActionError(
          caught instanceof Error ? caught.message : 'Project creation failed',
        )
        await loadProjects().catch(() => undefined)
      } finally {
        setAdoptingWorkspace(null)
      }
    },
    [
      inventory,
      loadInventory,
      loadProjects,
      loadWorkers,
      profiles,
      projects,
      selectedSession,
    ],
  )

  const provisionCentralOrchestrator = useCallback(
    async (profile: WorkerProfile) => {
      if (!yardOrchestrator) return
      setYardOrchestratorBusy(true)
      setActionError(null)
      try {
        const result = await provisionYardOrchestrator({
          command_id: crypto.randomUUID(),
          actor: 'local-user',
          profile_id: profile.id,
          expected_profile_version: profile.version,
          expected_orchestrator_version: yardOrchestrator.version,
        })
        setYardOrchestrator(result.orchestrator)
        setSelection({ kind: 'yard-orchestrator' })
        await Promise.all([
          loadSessions(),
          loadWorkers(),
          loadYardOrchestrator(),
        ])
      } catch (caught) {
        setActionError(
          caught instanceof Error
            ? caught.message
            : 'Superintendent provisioning failed',
        )
        await Promise.all([
          loadWorkers(),
          loadYardOrchestrator(),
        ]).catch(() => undefined)
      } finally {
        setYardOrchestratorBusy(false)
      }
    },
    [loadSessions, loadWorkers, loadYardOrchestrator, yardOrchestrator],
  )

  const recoverCentralOrchestrator = useCallback(async () => {
    if (!yardOrchestrator?.worker) return
    setYardOrchestratorBusy(true)
    setActionError(null)
    try {
      const result = await recoverYardOrchestrator({
        command_id: crypto.randomUUID(),
        actor: 'local-user',
        expected_orchestrator_version: yardOrchestrator.version,
      })
      setYardOrchestrator(result.orchestrator)
      await Promise.all([
        loadSessions(),
        loadWorkers(),
        loadYardOrchestrator(),
      ])
      setActionNotice('Superintendent session restarted.')
    } catch (caught) {
      setActionError(
        caught instanceof Error
          ? caught.message
          : 'Superintendent recovery failed',
      )
      await Promise.all([
        loadSessions(),
        loadWorkers(),
        loadYardOrchestrator(),
      ]).catch(() => undefined)
    } finally {
      setYardOrchestratorBusy(false)
    }
  }, [
    loadSessions,
    loadWorkers,
    loadYardOrchestrator,
    yardOrchestrator,
  ])

  const createNewWorkspaceProject = useCallback(
    async (details: WorkspaceProjectCreationDetails) => {
      setWorkspaceProjectBusy(true)
      setActionError(null)
      try {
        const profile = profiles.find(
          (candidate) => candidate.id === details.profileId,
        )
        if (!profile) {
          throw new Error('The selected orchestrator profile is unavailable')
        }
        const result = await createProjectWithWorkspace({
          command_id: details.commandId,
          actor: 'local-user',
          name: details.name,
          runtime_adapter: 'herdr',
          runtime_session: selectedSession,
          workspace_label: details.name,
          cwd: details.cwd,
          profile_id: profile.id,
          expected_profile_version: profile.version,
          orchestrator_objective: details.objective,
          placement: nextProjectPlacement(projects, 1),
        })
        setProjects((current) => [
          ...current.filter(
            (candidate) => candidate.id !== result.project.id,
          ),
          result.project,
        ])
        setSelection({ kind: 'project', id: result.project.id })
        setWorkspaceProjectOpen(false)
        await Promise.all([
          loadInventory(selectedSession),
          loadWorkers(),
        ])
      } catch (caught) {
        setActionError(
          caught instanceof Error ? caught.message : 'Project creation failed',
        )
        await loadProjects().catch(() => undefined)
      } finally {
        setWorkspaceProjectBusy(false)
      }
    },
    [
      loadInventory,
      loadProjects,
      loadWorkers,
      profiles,
      projects,
      selectedSession,
    ],
  )

  const flushPlacement = useCallback(
    async (projectId: string) => {
      if (placementUpdates.current.has(projectId)) return
      const currentProject = projectsRef.current.find(
        (candidate) => candidate.id === projectId,
      )
      if (!currentProject) return

      placementUpdates.current.add(projectId)
      let expectedVersion = currentProject.placement.version
      try {
        while (pendingPlacements.current.has(projectId)) {
          const placement = pendingPlacements.current.get(projectId)
          pendingPlacements.current.delete(projectId)
          if (!placement) break

          const updated = await updateProjectPlacement(projectId, {
            placement,
            expected_version: expectedVersion,
          })
          expectedVersion = updated.placement.version
          setProjects((current) => {
            const next = current.map((candidate) => {
              if (candidate.id !== updated.id) return candidate
              const queued = pendingPlacements.current.has(projectId)
              return queued
                ? {
                    ...updated,
                    placement: {
                      ...updated.placement,
                      geometry: candidate.placement.geometry,
                    },
                  }
                : updated
            })
            projectsRef.current = next
            return next
          })
        }
      } catch (caught) {
        pendingPlacements.current.delete(projectId)
        setActionError(
          caught instanceof Error ? caught.message : 'Placement update failed',
        )
        await loadProjects().catch(() => undefined)
      } finally {
        placementUpdates.current.delete(projectId)
      }
    },
    [loadProjects],
  )

  const persistPlacement = useCallback(
    (project: Project, placement: CanvasPlacement) => {
      pendingPlacements.current.set(project.id, placement)
      setActionError(null)
      setProjects((current) => {
        const next = current.map((candidate) =>
          candidate.id === project.id
            ? {
                ...candidate,
                placement: {
                  ...candidate.placement,
                  geometry: placement,
                },
              }
            : candidate,
        )
        projectsRef.current = next
        return next
      })
      void flushPlacement(project.id)
    },
    [flushPlacement],
  )

  const connectProjects = useCallback(
    async (sourceProjectId: string, targetProjectId: string) => {
      if (
        sourceProjectId === targetProjectId ||
        projectRelationships.some(
          (relationship) =>
            relationship.source_project_id === sourceProjectId &&
            relationship.target_project_id === targetProjectId &&
            relationship.kind === 'depends_on',
        )
      ) {
        return
      }
      setActionError(null)
      try {
        const result = await createProjectRelationship({
          command_id: crypto.randomUUID(),
          actor: 'local-user',
          relationship_id: crypto.randomUUID(),
          source_project_id: sourceProjectId,
          target_project_id: targetProjectId,
          kind: 'depends_on',
        })
        setProjectRelationships((current) => [
          ...current,
          result.relationship,
        ])
      } catch (caught) {
        setActionError(
          caught instanceof Error
            ? caught.message
            : 'Project connection failed',
        )
        await loadCoordination().catch(() => undefined)
      }
    },
    [loadCoordination, projectRelationships],
  )

  const disconnectProjects = useCallback(
    async (relationship: ProjectRelationship) => {
      setActionError(null)
      try {
        await deleteProjectRelationship(relationship.id, {
          command_id: crypto.randomUUID(),
          actor: 'local-user',
          expected_version: relationship.version,
        })
        setProjectRelationships((current) =>
          current.filter(
            (candidate) => candidate.id !== relationship.id,
          ),
        )
      } catch (caught) {
        setActionError(
          caught instanceof Error
            ? caught.message
            : 'Project connection removal failed',
        )
        await loadCoordination().catch(() => undefined)
      }
    },
    [loadCoordination],
  )

  const recordYardRoute = useCallback((route: YardOrchestratorRoute) => {
    setYardOrchestratorRoutes((current) => [
      route,
      ...current.filter(
        (candidate) => candidate.command_id !== route.command_id,
      ),
    ])
  }, [])

  const replaceCoordinationNode = useCallback((node: CoordinationNode) => {
    setCoordinationNodes((current) => {
      const exists = current.some((candidate) => candidate.id === node.id)
      const next = exists
        ? current.map((candidate) =>
            candidate.id === node.id ? node : candidate,
          )
        : [...current, node]
      coordinationNodesRef.current = next
      return next
    })
  }, [])

  const createMapNode = useCallback(
    async (details: CoordinationNodeCreationDetails) => {
      setCoordinationNodeBusy(true)
      setCoordinationNodeError(null)
      setActionError(null)
      try {
        const created = await createCoordinationNode({
          command_id: crypto.randomUUID(),
          actor: 'local-user',
          name: details.name,
          kind: details.kind,
          placement: details.placement,
          attached_project_ids: details.attachedProjectIds,
        })
        let node = created.node
        replaceCoordinationNode(node)
        setCoordinationSnapshots((current) => ({
          ...current,
          [node.id]: [],
        }))
        setSelection({ kind: 'coordination-node', id: node.id })
        setCoordinationNodePlacement(null)
        if (details.kind === 'workstream' && details.profileId) {
          const profile = profiles.find(
            (candidate) => candidate.id === details.profileId,
          )
          if (!profile) {
            setActionError(
              'The node was created, but the selected orchestrator profile is unavailable.',
            )
            return
          }
          try {
            const provisioned = await provisionCoordinationNode(node.id, {
              command_id: crypto.randomUUID(),
              actor: 'local-user',
              profile_id: profile.id,
              expected_profile_version: profile.version,
              expected_node_version: node.version,
            })
            node = provisioned.node
            replaceCoordinationNode(node)
          } catch (caught) {
            setActionError(
              caught instanceof Error
                ? `The node was created, but its worker could not be provisioned: ${caught.message}`
                : 'The node was created, but its worker could not be provisioned.',
            )
            await loadCoordination().catch(() => undefined)
            return
          }
        }
        if (node.worker) {
          await Promise.all([loadSessions(), loadWorkers()])
        }
      } catch (caught) {
        const message =
          caught instanceof Error
            ? caught.message
            : 'Coordination node creation failed'
        setCoordinationNodeError(message)
        await loadCoordination().catch(() => undefined)
      } finally {
        setCoordinationNodeBusy(false)
      }
    },
    [
      loadCoordination,
      loadSessions,
      loadWorkers,
      profiles,
      replaceCoordinationNode,
    ],
  )

  const saveCoordinationNode = useCallback(
    async (
      node: CoordinationNode,
      name: string,
      attachedProjectIds: string[],
    ) => {
      setCoordinationNodeBusy(true)
      setActionError(null)
      try {
        const result = await updateCoordinationNode(node.id, {
          command_id: crypto.randomUUID(),
          actor: 'local-user',
          expected_version: node.version,
          name,
          attached_project_ids: attachedProjectIds,
        })
        replaceCoordinationNode(result.node)
      } catch (caught) {
        setActionError(
          caught instanceof Error
            ? caught.message
            : 'Coordination node update failed',
        )
        await loadCoordination().catch(() => undefined)
      } finally {
        setCoordinationNodeBusy(false)
      }
    },
    [loadCoordination, replaceCoordinationNode],
  )

  const connectCoordinationNode = useCallback(
    async (nodeId: string, projectId: string) => {
      const node = coordinationNodesRef.current.find(
        (candidate) => candidate.id === nodeId,
      )
      if (!node || node.attached_project_ids.includes(projectId)) return
      await saveCoordinationNode(node, node.name, [
        ...node.attached_project_ids,
        projectId,
      ])
    },
    [saveCoordinationNode],
  )

  const provisionMapNode = useCallback(
    async (node: CoordinationNode, profile: WorkerProfile) => {
      setCoordinationNodeBusy(true)
      setActionError(null)
      try {
        const result = await provisionCoordinationNode(node.id, {
          command_id: crypto.randomUUID(),
          actor: 'local-user',
          profile_id: profile.id,
          expected_profile_version: profile.version,
          expected_node_version: node.version,
        })
        replaceCoordinationNode(result.node)
        await Promise.all([loadSessions(), loadWorkers()])
      } catch (caught) {
        setActionError(
          caught instanceof Error
            ? caught.message
            : 'Workstream provisioning failed',
        )
        await loadCoordination().catch(() => undefined)
      } finally {
        setCoordinationNodeBusy(false)
      }
    },
    [
      loadCoordination,
      loadSessions,
      loadWorkers,
      replaceCoordinationNode,
    ],
  )

  const collectKnowledgeSnapshot = useCallback(
    async (node: CoordinationNode) => {
      setCoordinationNodeBusy(true)
      setActionError(null)
      try {
        const snapshot = await requestCoordinationSnapshot(node.id, {
          command_id: crypto.randomUUID(),
          actor: 'local-user',
          expected_node_version: node.version,
        })
        setCoordinationSnapshots((current) => ({
          ...current,
          [node.id]: [
            snapshot,
            ...(current[node.id] ?? []).filter(
              (candidate) => candidate.id !== snapshot.id,
            ),
          ],
        }))
        setActionNotice(
          `Knowledge collection requested from ${snapshot.progress.total} project${
            snapshot.progress.total === 1 ? '' : 's'
          }.`,
        )
      } catch (caught) {
        setActionError(
          caught instanceof Error
            ? caught.message
            : 'Knowledge snapshot request failed',
        )
        await loadCoordination().catch(() => undefined)
      } finally {
        setCoordinationNodeBusy(false)
      }
    },
    [loadCoordination],
  )

  const recordCoordinationNodeRoute = useCallback(
    (route: CoordinationNodeRoute) => {
      setCoordinationNodeRoutes((current) => [
        route,
        ...current.filter(
          (candidate) => candidate.command_id !== route.command_id,
        ),
      ])
    },
    [],
  )

  const flushCoordinationPlacement = useCallback(
    async (nodeId: string) => {
      if (coordinationPlacementUpdates.current.has(nodeId)) return
      const currentNode = coordinationNodesRef.current.find(
        (candidate) => candidate.id === nodeId,
      )
      if (!currentNode) return

      coordinationPlacementUpdates.current.add(nodeId)
      let expectedVersion = currentNode.placement.version
      try {
        while (pendingCoordinationPlacements.current.has(nodeId)) {
          const placement =
            pendingCoordinationPlacements.current.get(nodeId)
          pendingCoordinationPlacements.current.delete(nodeId)
          if (!placement) break
          const result = await updateCoordinationNodePlacement(nodeId, {
            command_id: crypto.randomUUID(),
            actor: 'local-user',
            expected_version: expectedVersion,
            placement,
          })
          expectedVersion = result.node.placement.version
          replaceCoordinationNode(result.node)
        }
      } catch (caught) {
        pendingCoordinationPlacements.current.delete(nodeId)
        setActionError(
          caught instanceof Error
            ? caught.message
            : 'Coordination node placement failed',
        )
        await loadCoordination().catch(() => undefined)
      } finally {
        coordinationPlacementUpdates.current.delete(nodeId)
      }
    },
    [loadCoordination, replaceCoordinationNode],
  )

  const persistCoordinationPlacement = useCallback(
    (node: CoordinationNode, placement: CanvasPlacement) => {
      pendingCoordinationPlacements.current.set(node.id, placement)
      setCoordinationNodes((current) => {
        const next = current.map((candidate) =>
          candidate.id === node.id
            ? {
                ...candidate,
                placement: {
                  ...candidate.placement,
                  geometry: placement,
                },
              }
            : candidate,
        )
        coordinationNodesRef.current = next
        return next
      })
      void flushCoordinationPlacement(node.id)
    },
    [flushCoordinationPlacement],
  )

  const replaceAutomation = useCallback((automation: Automation) => {
    setAutomations((current) => {
      const exists = current.some(
        (candidate) => candidate.id === automation.id,
      )
      const next = exists
        ? current.map((candidate) =>
            candidate.id === automation.id ? automation : candidate,
          )
        : [...current, automation]
      automationsRef.current = next
      return next
    })
  }, [])

  const reconcileAutomation = useCallback(
    async (automationId: string) => {
      const automation = await fetchAutomation(automationId)
      replaceAutomation(automation)
      return automation
    },
    [replaceAutomation],
  )

  const createScheduledAutomation = useCallback(
    async (
      details: AutomationDetails & { placement: CanvasPlacement },
    ) => {
      setAutomationBusy(true)
      setAutomationError(null)
      setActionError(null)
      try {
        const automation = await createAutomation({
          command_id: crypto.randomUUID(),
          actor: 'local-user',
          name: details.name,
          placement: details.placement,
          prompt_template: details.promptTemplate,
          schedule: details.schedule,
          scope: details.scope,
          selected_project_ids: details.selectedProjectIds,
        })
        replaceAutomation(automation)
        setAutomationRuns((current) => ({
          ...current,
          [automation.id]: [],
        }))
        setAutomationCreation(null)
        setSelection({ kind: 'automation', id: automation.id })
      } catch (caught) {
        setAutomationError(
          caught instanceof Error
            ? caught.message
            : 'Automation creation failed',
        )
        await loadAutomations().catch(() => undefined)
      } finally {
        setAutomationBusy(false)
      }
    },
    [loadAutomations, replaceAutomation],
  )

  const saveAutomation = useCallback(
    async (automation: Automation, details: AutomationDetails) => {
      setAutomationBusy(true)
      setAutomationError(null)
      try {
        const updated = await updateAutomation(automation.id, {
          command_id: crypto.randomUUID(),
          actor: 'local-user',
          expected_version: automation.version,
          name: details.name,
          prompt_template: details.promptTemplate,
          schedule: details.schedule,
          scope: details.scope,
          selected_project_ids: details.selectedProjectIds,
        })
        replaceAutomation(updated)
      } catch (caught) {
        setAutomationError(
          caught instanceof Error
            ? caught.message
            : 'Automation update failed',
        )
        await reconcileAutomation(automation.id).catch(() => undefined)
      } finally {
        setAutomationBusy(false)
      }
    },
    [reconcileAutomation, replaceAutomation],
  )

  const toggleAutomationPaused = useCallback(
    async (automation: Automation) => {
      setAutomationBusy(true)
      setAutomationError(null)
      try {
        const updated = await updateAutomationState(automation.id, {
          command_id: crypto.randomUUID(),
          actor: 'local-user',
          expected_version: automation.version,
          paused: automation.state === 'active',
        })
        replaceAutomation(updated)
      } catch (caught) {
        setAutomationError(
          caught instanceof Error
            ? caught.message
            : 'Automation state update failed',
        )
        await reconcileAutomation(automation.id).catch(() => undefined)
      } finally {
        setAutomationBusy(false)
      }
    },
    [reconcileAutomation, replaceAutomation],
  )

  const requestAutomationRun = useCallback(
    async (automation: Automation) => {
      setAutomationBusy(true)
      setAutomationError(null)
      try {
        const run = await runAutomation(automation.id, {
          command_id: crypto.randomUUID(),
          actor: 'local-user',
          expected_version: automation.version,
        })
        setAutomationRuns((current) => ({
          ...current,
          [automation.id]: [
            run,
            ...(current[automation.id] ?? []).filter(
              (candidate) => candidate.id !== run.id,
            ),
          ],
        }))
        setAutomations((current) =>
          current.map((candidate) =>
            candidate.id === automation.id
              ? { ...candidate, latest_run: run }
              : candidate,
          ),
        )
        setActionNotice(
          run.status === 'submitted'
            ? 'Automation run submitted to transport. Completion is not implied.'
            : `Automation run is ${run.status}.`,
        )
        await reconcileAutomation(automation.id).catch(() => undefined)
      } catch (caught) {
        setAutomationError(
          caught instanceof Error
            ? caught.message
            : 'Automation run request failed',
        )
        await Promise.all([
          reconcileAutomation(automation.id),
          loadAutomationRuns(automation.id),
        ]).catch(() => undefined)
      } finally {
        setAutomationBusy(false)
      }
    },
    [loadAutomationRuns, reconcileAutomation],
  )

  const flushAutomationPlacement = useCallback(
    async (automationId: string) => {
      if (automationPlacementUpdates.current.has(automationId)) return
      const currentAutomation = automationsRef.current.find(
        (candidate) => candidate.id === automationId,
      )
      if (!currentAutomation) return

      automationPlacementUpdates.current.add(automationId)
      let expectedVersion = currentAutomation.placement.version
      try {
        while (pendingAutomationPlacements.current.has(automationId)) {
          const placement =
            pendingAutomationPlacements.current.get(automationId)
          pendingAutomationPlacements.current.delete(automationId)
          if (!placement) break
          const updated = await updateAutomationPlacement(automationId, {
            command_id: crypto.randomUUID(),
            actor: 'local-user',
            expected_version: expectedVersion,
            placement,
          })
          expectedVersion = updated.placement.version
          setAutomations((current) => {
            const next = current.map((candidate) => {
              if (candidate.id !== updated.id) return candidate
              return pendingAutomationPlacements.current.has(automationId)
                ? {
                    ...updated,
                    placement: {
                      ...updated.placement,
                      geometry: candidate.placement.geometry,
                    },
                  }
                : updated
            })
            automationsRef.current = next
            return next
          })
        }
      } catch (caught) {
        pendingAutomationPlacements.current.delete(automationId)
        setActionError(
          caught instanceof Error
            ? caught.message
            : 'Automation placement update failed',
        )
        await loadAutomations().catch(() => undefined)
      } finally {
        automationPlacementUpdates.current.delete(automationId)
      }
    },
    [loadAutomations],
  )

  const persistAutomationPlacement = useCallback(
    (automation: Automation, placement: CanvasPlacement) => {
      pendingAutomationPlacements.current.set(automation.id, placement)
      setAutomations((current) => {
        const next = current.map((candidate) =>
          candidate.id === automation.id
            ? {
                ...candidate,
                placement: {
                  ...candidate.placement,
                  geometry: placement,
                },
              }
            : candidate,
        )
        automationsRef.current = next
        return next
      })
      void flushAutomationPlacement(automation.id)
    },
    [flushAutomationPlacement],
  )

  const saveProfile = useCallback(
    async (spec: CreateWorkerProfileInput) => {
      if (profileEditor === undefined) return
      setProfileSaving(true)
      setActionError(null)
      try {
        const saved = profileEditor
          ? await updateWorkerProfile(profileEditor.id, {
              ...spec,
              expected_version: profileEditor.version,
            })
          : await createWorkerProfile(spec)
        setProfiles((current) => {
          const exists = current.some((profile) => profile.id === saved.id)
          return exists
            ? current.map((profile) =>
                profile.id === saved.id ? saved : profile,
              )
            : [...current, saved]
        })
        setSelection({ kind: 'profile', id: saved.id })
        setProfileEditor(undefined)
      } catch (caught) {
        setActionError(
          caught instanceof Error ? caught.message : 'Profile save failed',
        )
        await loadProfiles().catch(() => undefined)
      } finally {
        setProfileSaving(false)
      }
    },
    [loadProfiles, profileEditor],
  )

  const proposeAllocation = useCallback(
    (payload: AllocationDragPayload, projectId: string) => {
      const project = projects.find((candidate) => candidate.id === projectId)
      if (!project) return
      if (payload.kind === 'profile') {
        const profile = profiles.find(
          (candidate) => candidate.id === payload.id,
        )
        if (!profile) return
        setActionError(null)
        setAllocationError(null)
        setAllocationProposal({
          subject: { kind: 'profile', profile },
          project,
          commandId: crypto.randomUUID(),
        })
        return
      }

      const candidate = workerCandidates.find(
        ({ worker }) => worker.id === payload.id,
      )
      if (!candidate) return
      if (canAllocateCandidate(candidate)) {
        setActionError(null)
        setAllocationError(null)
        setAllocationProposal({
          subject: { kind: 'worker', candidate },
          project,
          commandId: crypto.randomUUID(),
        })
        return
      }
      if (!canHandoffCandidate(candidate)) return
      if (candidate.project_id === project.id) {
        setActionError('Select a different target project for the handoff.')
        return
      }
      const sourceAssignment = assignments.find(
        (assignment) =>
          assignment.id === candidate.assignment_id &&
          assignment.lifecycle === 'active',
      )
      const sourceProject = projects.find(
        (source) => source.id === candidate.project_id,
      )
      if (!sourceAssignment || !sourceProject) {
        setActionError('The active source assignment is no longer available.')
        return
      }
      setActionError(null)
      setHandoffError(null)
      setHandoffProposal({
        commandId: crypto.randomUUID(),
        sourceAssignment,
        sourceProject,
        targetProject: project,
      })
    },
    [assignments, profiles, projects, workerCandidates],
  )

  const allocateWorker = useCallback(
    async ({ objective, role, profileId }: AllocationDetails) => {
      if (!allocationProposal) return
      const selectedProfile = profileId
        ? profiles.find((profile) => profile.id === profileId)
        : undefined

      setAllocationBusy(true)
      setAllocationError(null)
      setActionError(null)
      try {
        const common = {
          command_id: allocationProposal.commandId,
          actor: 'local-user',
          expected_project_version: allocationProposal.project.version,
          objective,
          role,
          isolation_policy: 'project_workspace' as const,
        }
        const command =
          allocationProposal.subject.kind === 'profile'
            ? {
                ...common,
                profile_id: allocationProposal.subject.profile.id,
                expected_profile_version:
                  allocationProposal.subject.profile.version,
              }
            : {
                ...common,
                worker_id:
                  allocationProposal.subject.candidate.worker.id,
                expected_worker_version:
                  allocationProposal.subject.candidate.worker.version,
                ...(selectedProfile
                  ? {
                      profile_id: selectedProfile.id,
                      expected_profile_version: selectedProfile.version,
                    }
                  : {}),
              }
        const result = await confirmWorkerAssignment(
          allocationProposal.project.id,
          command,
        )
        setAssignments((current) => [
          ...current.filter(
            (assignment) => assignment.id !== result.assignment.id,
          ),
          result.assignment,
        ])
        setSelection({ kind: 'assignment', id: result.assignment.id })
        setAllocationError(null)
        setAllocationProposal(null)
        await Promise.all([
          loadProjects(),
          loadInventory(selectedSession),
          loadWorkers(),
        ])
      } catch (caught) {
        setAllocationError(
          caught instanceof Error ? caught.message : 'Worker allocation failed',
        )
        await Promise.all([loadProjects(), loadWorkers()]).catch(
          () => undefined,
        )
      } finally {
        setAllocationBusy(false)
      }
    },
    [
      allocationProposal,
      loadInventory,
      loadProjects,
      loadWorkers,
      profiles,
      selectedSession,
    ],
  )

  const handoffWorker = useCallback(
    async ({ objective, role, targetRole }: HandoffDetails) => {
      if (!handoffProposal) return
      const { commandId, sourceAssignment, sourceProject, targetProject } =
        handoffProposal
      setHandoffBusy(true)
      setHandoffError(null)
      setActionError(null)
      try {
        const result = await confirmWorkerHandoff(
          sourceProject.id,
          sourceAssignment.id,
          {
            command_id: commandId,
            actor: 'local-user',
            worker_id: sourceAssignment.worker.id,
            expected_worker_version: sourceAssignment.worker.version,
            expected_source_project_version: sourceProject.version,
            target_project_id: targetProject.id,
            expected_target_project_version: targetProject.version,
            source_attempt_id: sourceAssignment.attempt.id,
            expected_source_assignment_version: sourceAssignment.version,
            expected_source_attempt_version: sourceAssignment.attempt.version,
            target_role: targetRole,
            objective,
            role,
            isolation_policy: 'project_workspace',
          },
        )
        setAssignments((current) => [
          ...current.filter(
            (assignment) =>
              assignment.id !== result.source_assignment.id &&
              assignment.id !== result.assignment.id,
          ),
          result.source_assignment,
          result.assignment,
        ])
        setSelection({ kind: 'assignment', id: result.assignment.id })
        setHandoffProposal(null)
        await Promise.all([
          loadProjects(),
          loadInventory(selectedSession),
          loadWorkers(),
        ])
      } catch (caught) {
        setHandoffError(
          caught instanceof Error ? caught.message : 'Worker handoff failed',
        )
        await Promise.all([loadProjects(), loadWorkers()]).catch(
          () => undefined,
        )
      } finally {
        setHandoffBusy(false)
      }
    },
    [
      handoffProposal,
      loadInventory,
      loadProjects,
      loadWorkers,
      selectedSession,
    ],
  )

  const completeAssignment = useCallback(
    async (details: CompletionDetails) => {
      if (!completionProposal) return
      const assignment = completionProposal.assignment
      setCompletionBusy(true)
      setCompletionError(null)
      setActionError(null)
      try {
        const artifacts = await Promise.all(
          details.artifacts.map(async ({ file, id, kind }) => {
            const content = decodeUtf8(await file.arrayBuffer())
            return uploadArtifact(assignment.project_id, assignment.id, id, {
              actor: 'local-user',
              attempt_id: assignment.attempt.id,
              expected_assignment_version: assignment.version,
              expected_attempt_version: assignment.attempt.version,
              kind,
              display_name: file.name,
              content,
            })
          }),
        )
        const result = await recordCompletionReceipt(
          assignment.project_id,
          assignment.id,
          {
            command_id: completionProposal.commandId,
            actor: 'local-user',
            attempt_id: assignment.attempt.id,
            expected_assignment_version: assignment.version,
            expected_attempt_version: assignment.attempt.version,
            outcome: 'completed',
            summary: details.summary,
            artifact_refs: details.artifactRefs,
            artifact_ids: artifacts.map((artifact) => artifact.id),
            evidence_refs: details.evidenceRefs,
            unresolved_blockers: details.unresolvedBlockers,
          },
        )
        setAssignments((current) =>
          current.map((assignment) =>
            assignment.id === result.assignment.id
              ? result.assignment
              : assignment,
          ),
        )
        setSelection({ kind: 'assignment', id: result.assignment.id })
        setCompletionError(null)
        setCompletionProposal(null)
        await loadWorkers().catch(() => undefined)
      } catch (caught) {
        setCompletionError(
          caught instanceof Error
            ? caught.message
            : 'Completion receipt failed',
        )
        const loaded = await loadProjects().catch(() => null)
        const reconciled = loaded?.find(
          (candidate) => candidate.id === assignment.id,
        )
        if (reconciled?.completion_receipt) {
          setCompletionError(null)
          setSelection({ kind: 'assignment', id: reconciled.id })
          setCompletionProposal(null)
        }
      } finally {
        setCompletionBusy(false)
      }
    },
    [completionProposal, loadProjects, loadWorkers],
  )

  const resolvedProjectAccents = useMemo(
    () =>
      Object.fromEntries(
        projects.map((project) => [
          project.id,
          projectAccents[project.id] ?? defaultProjectAccent(project.id),
        ]),
      ),
    [projectAccents, projects],
  )
  const setProjectAccent = useCallback(
    (projectId: string, accent: string) => {
      setProjectAccents((current) => {
        const next = { ...current, [projectId]: accent }
        writeProjectAccents(next)
        return next
      })
    },
    [],
  )
  const toggleResourceShelf = useCallback(
    (view: RailView) => {
      setRailView(view)
      setResourceShelfOpen(
        (current) => view !== railView || !current,
      )
    },
    [railView],
  )
  const handleCanvasSelectionChange = useCallback(
    (nextSelection: CanvasSelection) => {
      setWorkspaceProjectOpen(false)
      setSelection(nextSelection)
    },
    [],
  )

  const displayedError = actionError ?? runtimeError
  const runtimeUnavailable =
    !runtimeLoading && inventory === null
  const runtimeHealth = runtimeLoading
    ? 'loading'
    : runtimeUnavailable
      ? 'unavailable'
      : runtimeError
        ? 'degraded'
        : 'observed'

  return (
    <AgentWorkspaceContext.Provider value={agentWorkspaceContext}>
      <div className="app-shell" data-shelf-open={resourceShelfOpen}>
        <GlobalCommandBar
          activeAgentWorkspaceChat={Boolean(
            activeAgentWorkspaceTarget?.chatAvailable,
          )}
          activeAgentWorkspaceTerminal={Boolean(
            activeAgentWorkspaceTarget?.interactive,
          )}
          activeAgentWorkspaceTarget={Boolean(activeAgentWorkspaceTarget)}
          agentWorkspaceMode={agentWorkspaceMode}
          busy={runtimeLoading || projectLoading}
          health={runtimeHealth}
          onAgentWorkspaceModeChange={(mode) => {
            if (
              (mode === 'chat' &&
                !activeAgentWorkspaceTarget?.chatAvailable) ||
              (mode === 'terminal' &&
                !activeAgentWorkspaceTarget?.interactive)
            ) {
              return
            }
            if (mode === 'terminal') {
              setTerminalPresentation(DEFAULT_TERMINAL_PRESENTATION)
            }
            setAgentWorkspaceMode(mode)
          }}
          onCreateProject={() => {
            setSelection(null)
            setWorkspaceProjectOpen(true)
          }}
          onOpenSettings={() => setSettingsOpen(true)}
          onRefresh={() => void refresh()}
          onResourceViewChange={toggleResourceShelf}
          onSessionChange={setSelectedSession}
          railView={railView}
          resourceShelfOpen={resourceShelfOpen}
          selectedSession={selectedSession}
          sessions={sessions}
          settingsLabel={`Settings, ${themeDefinition(theme).label} theme, ${
            mapVisualMode === 'depth' ? '2.5D' : '2D'
          } map`}
          settingsTriggerRef={settingsTrigger}
        />

        {resourceShelfOpen ? (
          <section
            aria-label={`${railView} shelf`}
            className="resource-shelf"
            id="resource-shelf"
          >
          <button
            aria-label="Collapse resource shelf"
            className="icon-button resource-shelf__collapse"
            onClick={() => setResourceShelfOpen(false)}
            title="Collapse resource shelf"
            type="button"
          >
            <ChevronUp aria-hidden="true" size={16} />
          </button>

        {railView === 'profiles' ? (
          <div className="rail-section rail-section--resources" role="tabpanel">
            <div className="section-heading">
              <div>
                <p className="eyebrow">Reusable</p>
                <h2>Profiles</h2>
              </div>
              <button
                aria-label="Create worker profile"
                className="icon-button section-heading__action"
                onClick={() => setProfileEditor(null)}
                title="New profile"
                type="button"
              >
                <Plus aria-hidden="true" size={16} />
              </button>
            </div>
            <div className="resource-list profile-list">
              <div className="profile-list__mobile-actions">
                <button
                  aria-label="Create worker profile"
                  className="icon-button"
                  onClick={() => setProfileEditor(null)}
                  title="New profile"
                  type="button"
                >
                  <Plus aria-hidden="true" size={16} />
                </button>
              </div>
              {profiles.map((profile) => (
                <button
                  className="profile-row"
                  data-selected={
                    selection?.kind === 'profile' &&
                    selection.id === profile.id
                  }
                  draggable
                  key={profile.id}
                  onClick={() =>
                    setSelection({ kind: 'profile', id: profile.id })
                  }
                  onDragStart={(event) => {
                    setAllocationDragData(event.dataTransfer, {
                      kind: 'profile',
                      id: profile.id,
                    })
                  }}
                  type="button"
                >
                  <span className="profile-row__grip">
                    <GripVertical aria-hidden="true" size={14} />
                  </span>
                  <span>
                    <strong>{profile.name}</strong>
                    <small>
                      {profile.provider}
                      {profile.model ? ` / ${profile.model}` : ''}
                    </small>
                  </span>
                </button>
              ))}
              {!projectLoading && profiles.length === 0 ? (
                <div className="empty-state profile-empty">
                  <p>No worker profiles.</p>
                  <button
                    className="secondary-button"
                    onClick={() => setProfileEditor(null)}
                    type="button"
                  >
                    <Plus aria-hidden="true" size={15} />
                    New profile
                  </button>
                </div>
              ) : null}
            </div>
          </div>
        ) : railView === 'workers' ? (
          <div className="rail-section rail-section--resources" role="tabpanel">
            <div className="section-heading">
              <div>
                <p className="eyebrow">Allocation</p>
                <h2>Workers</h2>
              </div>
              <span>{visibleCandidates.length}</span>
            </div>
            <div className="segmented-control" aria-label="Filter workers">
              {FILTERS.map((option) => (
                <button
                  aria-pressed={filter === option.value}
                  key={option.value}
                  onClick={() => setFilter(option.value)}
                  type="button"
                >
                  {option.label}
                </button>
              ))}
            </div>
            <div className="resource-list worker-list">
              {visibleCandidates.map((candidate) => {
                const capabilities = resolveRuntimeCapabilities(
                  inventoryCurrent,
                  candidate.worker.runtime,
                  inventory,
                )
                const observed = capabilities.observedWorker
                const runtimeState = resolvedRuntimeState(
                  candidate.worker.runtime,
                  capabilities,
                )
                const StatusIcon = STATUS_ICONS[runtimeState.status]
                const draggable =
                  canAllocateCandidate(candidate) ||
                  canHandoffCandidate(candidate)
                return (
                  <button
                    className="worker-row"
                    data-availability={candidate.availability}
                    data-draggable={draggable}
                    data-process-state={runtimeState.processState}
                    data-status={runtimeState.status}
                    data-worker-id={candidate.worker.id}
                    data-selected={
                      selection?.kind === 'worker' &&
                      selection.id === candidate.worker.id
                    }
                    draggable={draggable}
                    key={candidate.worker.id}
                    onClick={() =>
                      setSelection({
                        kind: 'worker',
                        id: candidate.worker.id,
                      })
                    }
                    onDragStart={(event) => {
                      if (!draggable) {
                        event.preventDefault()
                        return
                      }
                      setAllocationDragData(event.dataTransfer, {
                        kind: 'worker',
                        id: candidate.worker.id,
                        mode: canHandoffCandidate(candidate)
                          ? 'handoff'
                          : 'allocate',
                      })
                    }}
                    type="button"
                  >
                    <span
                      className="worker-row__status"
                      data-status={runtimeState.status}
                    >
                      <StatusIcon
                        aria-hidden="true"
                        className={
                          runtimeState.status === 'working' ? 'status-spin' : ''
                        }
                        size={14}
                      />
                    </span>
                    <span>
                      <strong>{candidateLabel(candidate)}</strong>
                      <small>
                        {AVAILABILITY_LABELS[candidate.availability]}
                        {' / '}
                        {runtimeState.status}
                        {' / '}
                        {observed?.provider ?? candidate.default_role ?? 'no profile'}
                      </small>
                    </span>
                  </button>
                )
              })}
              {!projectLoading && visibleCandidates.length === 0 ? (
                <p className="empty-state">
                  No worker candidates in this view.
                </p>
              ) : null}
            </div>
          </div>
        ) : (
          <div className="rail-section rail-section--resources" role="tabpanel">
            <div className="section-heading">
              <div>
                <p className="eyebrow">Unbound</p>
                <h2>Workspaces</h2>
              </div>
              <span>{availableWorkspaces.length}</span>
            </div>
            <div className="resource-list workspace-list">
              {availableWorkspaces.map((workspace) => (
                <button
                  className="workspace-row"
                  data-selected={
                    selection?.kind === 'workspace' &&
                    selection.id === workspace.runtime_id
                  }
                  key={workspace.runtime_id}
                  onClick={() =>
                    setSelection({
                      kind: 'workspace',
                      id: workspace.runtime_id,
                    })
                  }
                  type="button"
                >
                  <span
                    className="worker-row__status"
                    data-status={workspace.status}
                  >
                    <Boxes aria-hidden="true" size={14} />
                  </span>
                  <span>
                    <strong>{workspace.label}</strong>
                    <small>
                      {workspace.tab_count} tabs / {workspace.pane_count} panes
                    </small>
                  </span>
                </button>
              ))}
              {!runtimeLoading && availableWorkspaces.length === 0 ? (
                <p className="empty-state">No unbound workspaces.</p>
              ) : null}
            </div>
          </div>
        )}
          </section>
        ) : null}

        <main className="canvas-stage">
        <RuntimeCanvas
          allocationPayloadByRuntimeId={allocationPayloadByRuntimeId}
          assignments={assignments}
          automations={automations}
          coordinationNodes={coordinationNodes}
          coordinationNodeRoutes={coordinationNodeRoutes}
          inventory={inventory}
          onAllocationDrop={proposeAllocation}
          onAutomationPlacementChange={persistAutomationPlacement}
          onCoordinationNodeConnect={(nodeId, projectId) =>
            void connectCoordinationNode(nodeId, projectId)
          }
          onCoordinationNodePlacementChange={persistCoordinationPlacement}
          onCreateCoordinationNode={(placement, kind) => {
            setCoordinationNodeError(null)
            setCoordinationNodeInitialKind(kind)
            setCoordinationNodePlacement(placement)
          }}
          onCreateAutomation={(placement, initialScope) => {
            setAutomationError(null)
            setAutomationCreation({ initialScope, placement })
          }}
          onOpenProjectPulse={(trigger) => {
            projectPulseTrigger.current = trigger
            setProjectPulseOpen(true)
          }}
          onProjectConnect={(sourceProjectId, targetProjectId) =>
            void connectProjects(sourceProjectId, targetProjectId)
          }
          onProjectPlacementChange={persistPlacement}
          onSelectionChange={handleCanvasSelectionChange}
          projectAccents={resolvedProjectAccents}
          projectRelationships={projectRelationships}
          projectStatusReports={projectStatusReports}
          projects={projects}
          runtimeLoading={runtimeLoading}
          runtimeTopology={runtimeTopology}
          selectedSession={selectedSession}
          theme={themeDefinition(theme)}
          visualMode={mapVisualMode}
          visibleWorkers={visibleWorkers}
          yardOrchestrator={yardOrchestrator}
          yardOrchestratorRoutes={yardOrchestratorRoutes}
        />

        {!projectLoading &&
        !runtimeLoading &&
        projects.length === 0 &&
        coordinationNodes.length === 0 &&
        automations.length === 0 &&
        visibleWorkers.length === 0 ? (
          <div className="canvas-empty">
            <BriefcaseBusiness aria-hidden="true" size={28} />
            <strong>No Yard projects</strong>
          </div>
        ) : null}

        {displayedError ? (
          <div className="error-banner" role="alert">
            <CircleAlert aria-hidden="true" size={18} />
            <span>{displayedError}</span>
            <button
              aria-label="Dismiss error"
              className="icon-button"
              onClick={() => {
                if (actionError) setActionError(null)
                else setRuntimeError(null)
              }}
              title="Dismiss"
              type="button"
            >
              <X aria-hidden="true" size={16} />
            </button>
          </div>
        ) : actionNotice ? (
          <div className="notice-banner" role="status">
            <CircleAlert aria-hidden="true" size={18} />
            <span>{actionNotice}</span>
            <button
              aria-label="Dismiss notice"
              className="icon-button"
              onClick={() => setActionNotice(null)}
              title="Dismiss"
              type="button"
            >
              <X aria-hidden="true" size={16} />
            </button>
          </div>
        ) : null}
        {selection || workspaceProjectOpen ? (
          <aside className="inspector has-selection">
          <button
            aria-label="Close details"
            className="icon-button inspector__close"
            onClick={() => {
              setSelection(null)
              setWorkspaceProjectOpen(false)
            }}
            title="Close details"
            type="button"
          >
            <X aria-hidden="true" size={16} />
          </button>
        {selectedAgentGroup.length > 1 ? (
          <AgentGroupChat targets={selectedAgentGroup} />
        ) : selectedAutomation ? (
          <AutomationInspector
            automation={selectedAutomation}
            busy={automationBusy}
            coordinationNodes={coordinationNodes}
            error={automationError}
            historyLoading={automationHistoryLoading}
            key={`${selectedAutomation.id}:${selectedAutomation.version}`}
            onRun={(automation) => void requestAutomationRun(automation)}
            onSave={(automation, details) =>
              void saveAutomation(automation, details)
            }
            onTogglePaused={(automation) =>
              void toggleAutomationPaused(automation)
            }
            projects={projects}
            runs={automationRuns[selectedAutomation.id] ?? []}
          />
        ) : selectedYardOrchestrator ? (
          <YardOrchestratorInspector
            busy={yardOrchestratorBusy}
            inventory={inventory}
            onCoordinationChange={recordYardRoute}
            onProvision={provisionCentralOrchestrator}
            onRecover={() => void recoverCentralOrchestrator()}
            onRefresh={() => void refresh()}
            orchestrator={selectedYardOrchestrator}
            profiles={profiles}
            projects={projects}
            routes={yardOrchestratorRoutes}
            sessionRunning={
              sessions.find(
                (session) => session.name === 'yard-orchestrator',
              )?.running
            }
            snapshotCurrent={inventoryCurrent}
          />
        ) : selectedCoordinationNode ? (
          <CoordinationNodeInspector
            busy={coordinationNodeBusy}
            inventory={inventory}
            node={selectedCoordinationNode}
            onProvision={(node, profile) =>
              void provisionMapNode(node, profile)
            }
            onRouteChange={recordCoordinationNodeRoute}
            onSnapshot={(node) => void collectKnowledgeSnapshot(node)}
            onUpdate={(node, name, attachedProjectIds) =>
              void saveCoordinationNode(node, name, attachedProjectIds)
            }
            profiles={profiles}
            projects={projects}
            routes={coordinationNodeRoutes.filter(
              (route) => route.node_id === selectedCoordinationNode.id,
            )}
            snapshotCurrent={inventoryCurrent}
            snapshots={
              coordinationSnapshots[selectedCoordinationNode.id] ?? []
            }
          />
        ) : selectedProjectOrchestrator ? (
          <ProjectOrchestratorInspector
            candidates={selectedProjectOrchestratorEligibility.candidates}
            eligibilityReason={
              selectedProjectOrchestratorEligibility.reason
            }
            inventory={selectedProjectTransferSnapshot?.inventory ?? null}
            inventoryError={
              selectedProjectTransferContext?.error ?? null
            }
            inventoryLoading={
              selectedProjectTransferContext?.loading ?? true
            }
            onChange={(trigger) =>
              selectedProjectOrchestratorContextProject &&
              proposeProjectOrchestratorTransfer(
                selectedProjectOrchestratorContextProject,
                trigger,
              )
            }
            onRefresh={() => void refresh()}
            project={
              selectedProjectOrchestratorContextProject ??
              selectedProjectOrchestrator
            }
            statusReport={
              projectStatusReports[selectedProjectOrchestrator.id]
            }
          />
        ) : selectedProject ? (
          <ProjectInspector
            accent={resolvedProjectAccents[selectedProject.id]}
            inventory={inventory}
            onAccentChange={(accent) =>
              setProjectAccent(selectedProject.id, accent)
            }
            onArchive={(trigger) =>
              proposeProjectArchive(selectedProject, trigger)
            }
            onDelete={(trigger) =>
              proposeProjectDelete(selectedProject, trigger)
            }
            onDisconnect={(relationship) =>
              void disconnectProjects(relationship)
            }
            onRefresh={() => void refresh()}
            project={selectedProject}
            projects={projects}
            relationships={projectRelationships.filter(
              (relationship) =>
                relationship.source_project_id === selectedProject.id ||
                relationship.target_project_id === selectedProject.id,
            )}
            snapshotCurrent={inventoryCurrent}
            statusReport={projectStatusReports[selectedProject.id]}
          />
        ) : selectedAssignment ? (
          <AssignmentInspector
            assignment={selectedAssignment}
            inventory={inventory}
            onDelete={
              selectedAssignmentCandidate &&
              canDeleteCandidate(selectedAssignmentCandidate)
                ? () => proposeWorkerDelete(selectedAssignmentCandidate)
                : undefined
            }
            onEndSession={
              selectedAssignmentCandidate &&
              canEndCandidate(selectedAssignmentCandidate)
                ? () => proposeEndSession(selectedAssignmentCandidate)
                : undefined
            }
            onRecordCompletion={() =>
              setCompletionProposal({
                assignment: selectedAssignment,
                commandId: crypto.randomUUID(),
              })
            }
            onRefresh={() => void refresh()}
            snapshotCurrent={inventoryCurrent}
          />
        ) : selectedProfile ? (
          <ProfileInspector
            onAllocate={(project) =>
              proposeAllocation(
                { kind: 'profile', id: selectedProfile.id },
                project.id,
              )
            }
            onEdit={() => setProfileEditor(selectedProfile)}
            profile={selectedProfile}
            projects={projects}
          />
        ) : selectedWorkerCandidate ? (
          <WorkerCandidateInspector
            activeAssignment={selectedCandidateActiveAssignment}
            candidate={selectedWorkerCandidate}
            completedAssignment={selectedCandidateCompletion}
            inventory={inventory}
            onAllocate={(project) =>
              proposeAllocation(
                { kind: 'worker', id: selectedWorkerCandidate.worker.id },
                project.id,
              )
            }
            onEndSession={() =>
              proposeEndSession(selectedWorkerCandidate)
            }
            onDelete={() =>
              proposeWorkerDelete(selectedWorkerCandidate)
            }
            onRefresh={() => void refresh()}
            projects={projects}
            snapshotCurrent={inventoryCurrent}
          />
        ) : selectedObservedWorker ? (
          <ObservedWorkerInspector worker={selectedObservedWorker} />
        ) : selectedProviderChild ? (
          <ProviderChildInspector agent={selectedProviderChild} />
        ) : selectedWorkspace ? (
          <WorkspaceInspector
            busy={adoptingWorkspace === selectedWorkspace.runtime_id}
            key={selectedWorkspace.runtime_id}
            onCreate={(details) =>
              createWorkspaceProject(selectedWorkspace, details)
            }
            profiles={profiles}
            workers={selectedWorkspaceWorkers}
            workspace={selectedWorkspace}
          />
        ) : (
          <NewProjectInspector
            busy={workspaceProjectBusy}
            onCreate={createNewWorkspaceProject}
            onNewProfile={() => setProfileEditor(null)}
            profiles={profiles}
            session={selectedSession}
          />
        )}
          </aside>
        ) : null}
        </main>
      </div>
      {settingsOpen ? (
        <SettingsDialog
          automaticCoordinationEnabledCount={
            tokenSpendSettings
              ? Number(
                  tokenSpendSettings.superintendent_auto_requests_project_summaries,
                ) +
                Number(
                  tokenSpendSettings.project_orchestrators_auto_request_worker_summaries,
                ) +
                Number(tokenSpendSettings.scheduled_automatic_summaries)
              : null
          }
          mapVisualMode={mapVisualMode}
          onClose={() => setSettingsOpen(false)}
          onMapVisualModeChange={setMapVisualMode}
          onOpenAutomaticCoordination={() => {
            setSettingsOpen(false)
            setTokenSpendSettingsError(null)
            setTokenSpendSettingsOpen(true)
          }}
          onOpenOrchestratorWorkflow={() => {
            setSettingsOpen(false)
            setOrchestratorWorkflowError(null)
            setOrchestratorWorkflowOpen(true)
          }}
          onThemeChange={setTheme}
          returnFocus={settingsTrigger.current}
          theme={theme}
          workflowProfileSummary={
            orchestratorWorkflowProfile
              ? `Revision ${orchestratorWorkflowProfile.version} · ${
                  Number(orchestratorWorkflowProfile.monitor_interval_ms) /
                  60_000
                } minute cadence`
              : null
          }
        />
      ) : null}
      {activeAgentWorkspaceTarget ? (
        <AgentWorkspaceShell
          activeTarget={activeAgentWorkspaceTarget}
          coordinationRoutes={coordinationNodeRoutes}
          mode={agentWorkspaceMode}
          onCoordinationChange={recordYardRoute}
          onCoordinationNodeChange={recordCoordinationNodeRoute}
          onModeChange={setAgentWorkspaceMode}
          onPresentationChange={setTerminalPresentation}
          onRefresh={() => void refresh()}
          onTargetChange={(target) => {
            if (!target.interactive) return
            setAgentWorkspaceTarget({
              ...target,
              returnFocus: activeAgentWorkspaceTarget.returnFocus,
            })
            setTerminalPresentation(DEFAULT_TERMINAL_PRESENTATION)
            setAgentWorkspaceMode((current) =>
              current === 'changes'
                ? 'changes'
                : current === 'chat' && target.chatAvailable
                  ? 'chat'
                  : 'terminal',
            )
          }}
          presentation={terminalPresentation}
          inventory={inventory}
          inventoryCurrent={inventoryCurrent}
          projects={projects}
          sessions={sessions}
          targets={agentWorkspaceTargets}
          yardRoutes={yardOrchestratorRoutes}
        />
      ) : null}
      {projectPulseOpen ? (
        <ProjectPulseWorkspace
          assignments={assignments}
          coordinationNodes={coordinationNodes}
          coordinationSnapshots={coordinationSnapshots}
          inventory={inventory}
          onClose={() => setProjectPulseOpen(false)}
          onOpenCoordinationNode={(node) => {
            setProjectPulseOpen(false)
            setSelection({ kind: 'coordination-node', id: node.id })
          }}
          onOpenProject={(project) => {
            setProjectPulseOpen(false)
            setSelection({
              kind: 'orchestrator',
              projectId: project.id,
            })
          }}
          projects={projects}
          returnFocus={projectPulseTrigger.current}
          routes={yardOrchestratorRoutes}
          statusReports={projectStatusReports}
        />
      ) : null}
      {coordinationNodePlacement ? (
        <CoordinationNodeDialog
          busy={coordinationNodeBusy}
          error={coordinationNodeError}
          initialKind={coordinationNodeInitialKind}
          onClose={() => {
            if (coordinationNodeBusy) return
            setCoordinationNodeError(null)
            setCoordinationNodePlacement(null)
          }}
          onCreate={(details) => void createMapNode(details)}
          placement={coordinationNodePlacement}
          profiles={profiles}
          projects={projects}
        />
      ) : null}
      {automationCreation ? (
        <AutomationDialog
          busy={automationBusy}
          coordinationNodes={coordinationNodes}
          error={automationError}
          initialScope={automationCreation.initialScope}
          onClose={() => {
            if (automationBusy) return
            setAutomationError(null)
            setAutomationCreation(null)
          }}
          onCreate={(details) => void createScheduledAutomation(details)}
          placement={automationCreation.placement}
          projects={projects}
        />
      ) : null}
      {tokenSpendSettingsOpen && tokenSpendSettings ? (
        <TokenSpendSettingsDialog
          busy={tokenSpendSettingsBusy}
          error={tokenSpendSettingsError}
          key={tokenSpendSettings.version}
          onClose={() => {
            if (tokenSpendSettingsBusy) return
            setTokenSpendSettingsError(null)
            setTokenSpendSettingsOpen(false)
          }}
          onSave={(selection) => void saveTokenSpendSettings(selection)}
          returnFocus={settingsTrigger.current}
          settings={tokenSpendSettings}
        />
      ) : null}
      {orchestratorWorkflowOpen && orchestratorWorkflowProfile ? (
        <OrchestratorWorkflowProfileDialog
          busy={orchestratorWorkflowBusy}
          error={orchestratorWorkflowError}
          key={orchestratorWorkflowProfile.version}
          onClose={() => {
            if (orchestratorWorkflowBusy) return
            setOrchestratorWorkflowError(null)
            setOrchestratorWorkflowOpen(false)
          }}
          onReset={() => void resetOrchestratorWorkflow()}
          onSave={(input) => void saveOrchestratorWorkflow(input)}
          profile={orchestratorWorkflowProfile}
          returnFocus={settingsTrigger.current}
        />
      ) : null}
      {profileEditor !== undefined ? (
        <ProfileEditor
          busy={profileSaving}
          onClose={() => setProfileEditor(undefined)}
          onSave={saveProfile}
          profile={profileEditor}
        />
      ) : null}
      {allocationProposal ? (
        <AllocationDialog
          busy={allocationBusy}
          error={allocationError}
          onClose={() => {
            setAllocationError(null)
            setAllocationProposal(null)
          }}
          onConfirm={allocateWorker}
          profiles={profiles}
          project={allocationProposal.project}
          subject={allocationProposal.subject}
        />
      ) : null}
      {handoffProposal ? (
        <HandoffDialog
          busy={handoffBusy}
          error={handoffError}
          onClose={() => {
            setHandoffError(null)
            setHandoffProposal(null)
          }}
          onConfirm={handoffWorker}
          sourceAssignment={handoffProposal.sourceAssignment}
          sourceProject={handoffProposal.sourceProject}
          targetProject={handoffProposal.targetProject}
        />
      ) : null}
      {projectOrchestratorTransfer &&
      projectOrchestratorTransferProject ? (
        <ProjectOrchestratorTransferDialog
          busy={projectOrchestratorTransferBusy}
          candidates={projectOrchestratorTransferEligibility.candidates}
          error={projectOrchestratorTransferError}
          onClose={() => {
            if (projectOrchestratorTransferBusy) return
            setProjectOrchestratorTransferError(null)
            setProjectOrchestratorTransfer(null)
          }}
          onConfirm={transferProjectOrchestrator}
          project={projectOrchestratorTransferProject}
          returnFocus={projectOrchestratorTransfer.returnFocus}
        />
      ) : null}
      {completionProposal ? (
        <CompletionDialog
          assignment={completionProposal.assignment}
          busy={completionBusy}
          error={completionError}
          onClose={() => {
            setCompletionError(null)
            setCompletionProposal(null)
          }}
          onConfirm={completeAssignment}
        />
      ) : null}
      {projectArchiveProposal ? (
        <ArchiveProjectDialog
          activeAssignmentCount={projectArchiveActiveAssignmentCount}
          busy={projectArchiveBusy}
          error={projectArchiveError}
          onClose={() => {
            if (projectArchiveBusy) return
            setProjectArchiveError(null)
            setProjectArchiveProposal(null)
          }}
          onConfirm={archiveSelectedProject}
          project={projectArchiveProposal.project}
          returnFocus={projectArchiveProposal.returnFocus}
        />
      ) : null}
      {projectDeleteProposal ? (
        <DeleteProjectDialog
          activeAssignmentCount={projectDeleteActiveAssignmentCount}
          busy={projectDeleteBusy}
          error={projectDeleteError}
          onClose={() => {
            if (projectDeleteBusy) return
            setProjectDeleteError(null)
            setProjectDeleteProposal(null)
          }}
          onConfirm={deleteSelectedProject}
          project={projectDeleteProposal.project}
          returnFocus={projectDeleteProposal.returnFocus}
        />
      ) : null}
      {endSessionProposal ? (
        <EndWorkerSessionDialog
          busy={endSessionBusy}
          candidate={endSessionProposal.candidate}
          error={endSessionError}
          onClose={() => {
            if (endSessionBusy) return
            setEndSessionError(null)
            setEndSessionProposal(null)
          }}
          onConfirm={endSession}
        />
      ) : null}
      {workerDeleteProposal ? (
        <DeleteWorkerDialog
          busy={workerDeleteBusy}
          candidate={workerDeleteProposal.candidate}
          error={workerDeleteError}
          onClose={() => {
            if (workerDeleteBusy) return
            setWorkerDeleteError(null)
            setWorkerDeleteProposal(null)
          }}
          onConfirm={deleteSelectedWorker}
        />
      ) : null}
    </AgentWorkspaceContext.Provider>
  )
}

export default App

function decodeUtf8(buffer: ArrayBuffer) {
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(buffer)
  } catch {
    throw new Error('Artifact content must be UTF-8 text')
  }
}

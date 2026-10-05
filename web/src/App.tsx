import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type Dispatch,
  type FormEvent,
  type SetStateAction,
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
  RotateCcw,
  Server,
  Trash2,
  Unlink,
  Wifi,
  WifiOff,
  X,
} from 'lucide-react'
import {
  YardApiError,
  archiveCoordinationNode,
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
  deleteCoordinationNode,
  deleteProject,
  deleteProjectRelationship,
  deleteWorker,
  endWorkerSession,
  fetchCompletedRuntimeCleanupPreview,
  renameWorker,
  fetchAutomation,
  fetchAutomationRuns,
  fetchAutomations,
  fetchInventory,
  fetchRuntimeLens,
  fetchCoordinationNode,
  fetchCoordinationNodeDispositionPreview,
  fetchCoordinationNodeRoutes,
  fetchCoordinationNodes,
  fetchCoordinationSnapshots,
  fetchOrchestratorWorkflowProfile,
  fetchOrchestratorStatusOutput,
  fetchProject,
  fetchProjectAssignments,
  fetchProjectDispositionPreview,
  fetchSummaryWorkers,
  fetchProjectRelationships,
  fetchArchivedProjects,
  fetchProjects,
  fetchSessions,
  fetchTokenSpendSettings,
  fetchYardOrchestrator,
  fetchYardOrchestratorRoutes,
  fetchWorkerProfiles,
  fetchWorkers,
  provisionYardOrchestrator,
  receiveSummaryWorker,
  recoverYardOrchestrator,
  replaceProjectOrchestrator,
  resetOrchestratorWorkflowProfile,
  restoreProject,
  provisionCoordinationNode,
  recordCompletionReceipt,
  requestCoordinationSnapshot,
  requestSummaryWorker,
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
  ResourceShelfTabs,
  SettingsDialog,
  type ResourceView,
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
import { ProjectRepositoriesSection } from './RepositoryFilesWorkspace'
import { ProjectPulseWorkspace } from './ProjectPulseWorkspace'
import { HerdrInventoryWorkspace } from './HerdrInventoryWorkspace'
import {
  resolveRuntimeCapabilities,
  runtimeBindingReason,
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
import { OrchestratorWorkflowProfileDialog } from './OrchestratorWorkflowProfileDialog'
import {
  parseStatusReport,
  statusReportMatchesLatestRoute,
  type ProjectStatusReports,
} from './projectUpdates'
import { EndWorkerSessionDialog } from './EndWorkerSessionDialog'
import { CompletedRuntimeCleanupPreviewDialog } from './CompletedRuntimeCleanupPreviewDialog'
import { ArchiveProjectDialog } from './ArchiveProjectDialog'
import { DeleteProjectDialog } from './DeleteProjectDialog'
import { DeleteWorkerDialog } from './DeleteWorkerDialog'
import { HideStaleWorkersDialog } from './HideStaleWorkersDialog'
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
  labelWithDisplayName,
  workerDefaultLabel,
  workerDisplayLabel,
  workerLabelWithDefault,
  type WorkerLabelSource,
} from './workerDisplay'
import {
  workerAttentionState,
  workerCurrentlyObserved,
} from './workerAttention'
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
import {
  countSessionRoles,
  readStoredSession,
  resolveSelectedSession,
  writeStoredSession,
} from './sessionSelection'
import { useModalDialog } from './useModalDialog'
import {
  ARCHIVE_UNDO_WINDOW_MS,
  PROJECT_PREVIEW_REFRESH_CODES,
  PROJECT_VERSION_REFRESH_CODES,
  projectArchivePreconditions,
  projectDispositionEndedAssignments,
  projectDispositionNotice,
  projectRestoreErrorMessage,
  projectRestoreRetryable,
  projectRestoreNotice,
  workstreamDispositionNotice,
} from './backgroundStatus'
import {
  ArchivedProjectsPanel,
  type ProjectRestoreState,
} from './ArchivedProjectsPanel'
import { WorkstreamDispositionDialog } from './WorkstreamDispositionDialog'
import { AssignmentActions } from './AssignmentActions'
import { AssignmentTranscript } from './AssignmentTranscript'
import {
  WorkerDispositionSheet,
  type WorkerDispositionChoice,
  type WorkerDispositionMode,
} from './WorkerDispositionSheet'
import {
  DispositionCommandIds,
  dispositionNotice,
  submitDisposition,
  withLatestSessionVersions,
  type DispositionResult,
} from './assignmentDisposition'
import {
  readCompleteAndEndSession,
  writeCompleteAndEndSession,
} from './completionPreference'
import { useQuickCompletionWindow } from './useQuickCompletionWindow'
import type {
  Artifact,
  Assignment,
  AssignmentCancellation,
  AssignmentLifecycle,
  Automation,
  AutomationRun,
  AutomationScope,
  CanvasPlacement,
  CompletedRuntimeCleanupPreview,
  CoordinationNode,
  CoordinationNodeDispositionPreview,
  CoordinationNodeKind,
  CoordinationNodeRoute,
  CoordinationSnapshot,
  CreateWorkerProfileInput,
  DispositionOutcome,
  ObservedChildAgent,
  ObservedStatus,
  ObservedWorker,
  OrchestratorWorkflowProfile,
  Project,
  ArchivedProjectSummary,
  ProjectBackgroundStatus,
  ProjectDispositionPreview,
  ProjectRelationship,
  RuntimeInventory,
  RuntimeLensEntry,
  RuntimeObservationState,
  RuntimeProcessState,
  RuntimeSession,
  RuntimeTopology,
  StatusReport,
  SummaryWorker,
  TokenSpendSettings,
  WorkerAvailability,
  WorkerCandidate,
  WorkerProfile,
  WorkerRuntimeBinding,
  WorkflowStatus,
  WorkspaceObservation,
  YardOrchestrator,
  YardOrchestratorRoute,
  Worker,
} from './types'
import {
  workerDisplayName,
  withRenamedDisplayName,
  type NamedWorker,
} from './workerNames'
import {
  WorkerNameControl,
  type RenameWorkerHandler,
} from './WorkerNameControl'
import './App.css'

const INVENTORY_REFRESH_INTERVAL_MS = 1_000
const ASSIGNMENT_REFRESH_INTERVAL_MS = 5_000
const PROJECT_STATUS_REFRESH_INTERVAL_MS = 15_000
const MAX_STALE_HIDE_BATCH = 50

const ArtifactInspector = lazy(() =>
  import('./ArtifactInspector').then((module) => ({
    default: module.ArtifactInspector,
  })),
)

type Filter =
  | 'current'
  | 'available'
  | 'allocated'
  | 'attention'
  | 'stale'
  | 'history'

interface ProjectDispositionProposal {
  commandId: string
  preview: ProjectDispositionPreview | null
  previewError: string | null
  // The preview found no active project because it is already archived
  // (for example by another tab); Delete can still remove it.
  alreadyArchived?: boolean
  project: Project
  returnFocus: HTMLButtonElement | null
}

function isProjectDispositionPreview(
  value: unknown,
): value is ProjectDispositionPreview {
  return (
    typeof value === 'object' &&
    value !== null &&
    'project_id' in value &&
    Array.isArray((value as { active_assignments?: unknown }).active_assignments)
  )
}
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
  { value: 'stale', label: 'Stale' },
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
  coordination_node: Network,
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
  coordination_node: 'Coordination node',
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

function withoutKey<T>(record: Record<string, T>, key: string) {
  if (!(key in record)) return record
  const next = { ...record }
  delete next[key]
  return next
}

function matchesFilter(
  candidate: WorkerCandidate,
  filter: Filter,
  snapshotCurrent: boolean,
  inventory: RuntimeInventory | null,
  latestAssignmentLifecycle: AssignmentLifecycle | null = null,
) {
  if (filter === 'current') return candidate.availability !== 'ended'
  if (filter === 'history') return candidate.availability === 'ended'
  if (filter === 'stale') {
    return (
      workerAttentionState(
        candidate,
        snapshotCurrent,
        inventory,
        latestAssignmentLifecycle,
      ) === 'stale'
    )
  }
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
      candidate.availability === 'coordination_node' ||
      candidate.availability === 'assigned'
    )
  }
  if (filter === 'attention') {
    return (
      workerAttentionState(
        candidate,
        snapshotCurrent,
        inventory,
        latestAssignmentLifecycle,
      ) === 'actionable'
    )
  }
  return false
}

function workerLabel(
  worker: ObservedWorker,
  inventory?: RuntimeInventory | null,
  peerWorkerIds: string[] = [],
  displayName?: string | null,
) {
  return workerDisplayLabel(
    {
      displayName,
      observedDisplayProvider: worker.display_provider,
      observedName: worker.name,
      observedProvider: worker.provider,
      tabLabel: inventory?.tabs.find(
        (tab) => tab.runtime_id === worker.tab_id,
      )?.label,
      workerId: worker.runtime_id,
      workspaceLabel: inventory?.workspaces.find(
        (workspace) => workspace.runtime_id === worker.workspace_id,
      )?.label,
    },
    peerWorkerIds,
  )
}

function candidateLabel(candidate: WorkerCandidate) {
  return workerDisplayLabel({
    displayName: candidate.worker.display_name,
    profileName: candidate.profile_name,
    workerId: candidate.worker.id,
  })
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
    candidate.availability !== 'coordination_node' &&
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

async function hideWorkerCandidate(
  candidate: WorkerCandidate,
  endCommandId: string,
  deleteCommandId: string,
) {
  let expectedWorkerVersion = candidate.worker.version
  if (candidate.worker.desired_state !== 'ended') {
    const ended = await endWorkerSession(candidate.worker.id, {
      command_id: endCommandId,
      actor: 'local-user',
      expected_worker_version: candidate.worker.version,
      ...(candidate.worker.runtime
        ? { expected_runtime_version: candidate.worker.runtime.version }
        : {}),
    })
    expectedWorkerVersion = ended.worker.version
  }
  return deleteWorker(candidate.worker.id, {
    command_id: deleteCommandId,
    actor: 'local-user',
    expected_worker_version: expectedWorkerVersion,
  })
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
  capabilityDetail: string | null = null,
) {
  const observed = capabilities.observedWorker
  const current = observed ?? capabilities.observedPane
  return {
    capabilityDetail,
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

function workerLabelSource(
  candidate: WorkerCandidate,
  assignments: Assignment[],
  projects: Project[],
  inventory: RuntimeInventory | null,
  snapshotCurrent: boolean,
): WorkerLabelSource {
  const assignment = assignments.find(
    (item) =>
      item.worker.id === candidate.worker.id &&
      item.id === candidate.assignment_id,
  )
  const project = projects.find(
    (item) => item.id === (assignment?.project_id ?? candidate.project_id),
  )
  const observed = resolveRuntimeCapabilities(
    snapshotCurrent,
    candidate.worker.runtime,
    inventory,
  ).observedWorker
  const workspace = observed
    ? inventory?.workspaces.find(
        (item) => item.runtime_id === observed.workspace_id,
      )
    : null
  const tab = observed
    ? inventory?.tabs.find((item) => item.runtime_id === observed.tab_id)
    : null
  const orchestratorName =
    candidate.availability === 'yard_orchestrator'
      ? 'Yard'
      : candidate.availability === 'orchestrator'
        ? project?.name
        : null

  return {
    assignmentRole: assignment?.role,
    displayName: candidate.worker.display_name,
    observedDisplayProvider: observed?.display_provider,
    observedName: observed?.name,
    observedProvider: observed?.provider,
    profileName: orchestratorName ? null : candidate.profile_name,
    projectName: assignment ? project?.name : null,
    projectOrchestratorName: orchestratorName,
    tabLabel: tab?.label,
    workerId: candidate.worker.id,
    workspaceLabel: workspace?.label,
  }
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
  detail,
  state,
}: {
  capabilities: RuntimeCapabilities
  detail?: string
  state: RuntimeObservationState
}) {
  const Icon = OBSERVATION_ICONS[state]
  const label = runtimeCapabilityLabel(capabilities)
  const resolvedDetail = detail ?? runtimeCapabilityDetail(capabilities)
  return (
    <span
      aria-label={
        resolvedDetail
          ? `${label}. ${resolvedDetail}`
          : `Observation state: ${state}`
      }
      className="observation-state-badge"
      data-observation-state={
        capabilities.reason === 'stale' ? 'stale' : state
      }
      title={resolvedDetail}
    >
      <Icon aria-hidden="true" size={14} />
      {label}
    </span>
  )
}

function RuntimeStateSummary({
  compact = false,
  inventory,
  runtime,
  snapshotCurrent,
}: {
  compact?: boolean
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
  const capabilityDetail =
    runtimeBindingReason(snapshotCurrent, runtime, inventory) ??
    runtimeCapabilityDetail(capabilities)

  return (
    <dl
      className={`runtime-state-summary${
        compact ? ' runtime-state-summary--compact' : ''
      }`}
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
            detail={capabilityDetail}
            state={currentObservationState}
          />
        </dd>
      </div>
      {compact ? (
        <div className="runtime-state-summary__reason">
          <dt>Runtime detail</dt>
          <dd>{capabilityDetail ?? 'Live controls ready.'}</dd>
        </div>
      ) : null}
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

function ObservedWorkerInspector({
  label,
  worker,
}: {
  label: string
  worker: ObservedWorker
}) {
  return (
    <>
      <div className="inspector__identity">
        <span className="inspector__icon" data-status={worker.status}>
          <Bot aria-hidden="true" size={20} />
        </span>
        <div>
          <p className="eyebrow">Observed worker</p>
          <h2>{label}</h2>
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
  activeControls,
  candidate,
  completedAssignment,
  hideOnly,
  inventory,
  defaultLabel,
  onAllocate,
  onDelete,
  onEndSession,
  onRefresh,
  onRename,
  projects,
  snapshotCurrent,
}: {
  activeAssignment: Assignment | undefined
  activeControls: AssignmentDispositionControls | null
  candidate: WorkerCandidate
  completedAssignment: Assignment | undefined
  hideOnly: boolean
  inventory: RuntimeInventory | null
  // The label without the user-chosen name (rename field placeholder).
  defaultLabel: string
  onAllocate: (project: Project) => void
  onDelete: () => void
  onEndSession: () => void
  onRefresh: () => void
  onRename?: RenameWorkerHandler
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
          <p className="eyebrow">
            {candidate.default_role ?? 'Worker'}
          </p>
          <WorkerNameControl
            defaultLabel={defaultLabel}
            onRename={onRename}
            worker={candidate.worker}
          />
          <span
            className="availability-badge"
            data-availability={candidate.availability}
          >
            <AvailabilityIcon aria-hidden="true" size={12} />
            {AVAILABILITY_LABELS[candidate.availability]}
          </span>
        </div>
      </div>
      <RuntimeStateSummary
        compact
        inventory={inventory}
        runtime={candidate.worker.runtime}
        snapshotCurrent={snapshotCurrent}
      />
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
      {activeAssignment && activeControls ? (
        <ActiveAssignmentActions
          assignment={activeAssignment}
          controls={activeControls}
        />
      ) : null}
      {completedAssignment &&
      candidate.worker.desired_state === 'running' ? (
        <div className="awaiting-disposition" role="status">
          <CircleCheck aria-hidden="true" size={16} />
          <span>
            <strong>Awaiting disposition</strong>
            <small>
              {completedAssignment.lifecycle === 'cancelled'
                ? 'Work ended without completion. This session remains available for inspection or reassignment until you end it.'
                : 'Work is complete. This session remains available for inspection or reassignment until you end it.'}
            </small>
          </span>
        </div>
      ) : null}
      {!activeAssignment && completedAssignment ? (
        <AssignmentTranscript
          assignment={completedAssignment}
          key={`${completedAssignment.id}:${completedAssignment.version}`}
        />
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
            {hideOnly ? 'Hide stale' : 'Delete worker'}
          </button>
        </div>
      ) : null}
      <details className="worker-inspector-details">
        <summary>Details</summary>
        <dl className="detail-list">
          <DetailRow label="Worker ID" value={candidate.worker.id} mono />
          {candidate.profile_name ? (
            <DetailRow label="Profile" value={candidate.profile_name} />
          ) : null}
          {candidate.default_role ? (
            <DetailRow label="Default role" value={candidate.default_role} />
          ) : null}
          {observed?.provider ?? current?.provider ? (
            <DetailRow
              label="Provider"
              value={observed?.provider ?? current?.provider}
            />
          ) : null}
          {candidate.project_id ? (
            <DetailRow label="Project ID" value={candidate.project_id} mono />
          ) : null}
          {candidate.assignment_id ? (
            <DetailRow
              label="Assignment"
              value={candidate.assignment_id}
              mono
            />
          ) : null}
          {candidate.worker.runtime?.terminal_id ? (
            <DetailRow
              label="Terminal"
              value={candidate.worker.runtime.terminal_id}
              mono
            />
          ) : null}
          {candidate.worker.runtime?.state_change_sequence !== undefined ? (
            <DetailRow
              label="State sequence"
              value={candidate.worker.runtime.state_change_sequence}
              mono
            />
          ) : null}
          {candidate.worker.runtime?.revision !== undefined ? (
            <DetailRow
              label="Runtime revision"
              value={candidate.worker.runtime.revision}
              mono
            />
          ) : null}
          {candidate.reason ? (
            <DetailRow label="Reason" value={candidate.reason} />
          ) : null}
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
      </details>
    </>
  )
}

function YardOrchestratorInspector({
  busy,
  inventory,
  defaultLabel,
  label,
  onCoordinationChange,
  onProvision,
  onRecover,
  onRefresh,
  onRenameWorker,
  orchestrator,
  profiles,
  projects,
  routes,
  sessionRunning,
  snapshotCurrent,
}: {
  busy: boolean
  inventory: RuntimeInventory | null
  // The label without the user-chosen name (rename field placeholder).
  defaultLabel: string
  label: string
  onCoordinationChange: (route: YardOrchestratorRoute) => void
  onProvision: (profile: WorkerProfile) => void
  onRecover: () => void
  onRefresh: () => void
  onRenameWorker?: RenameWorkerHandler
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
  const bindingRecoveryRequired =
    capabilities.reason === 'binding_missing' ||
    capabilities.reason === 'identity_mismatch'
  const recoveryRequired =
    sessionStopped || agentStopped || bindingRecoveryRequired
  const bindingDetail = runtimeBindingReason(
    snapshotCurrent,
    worker?.runtime,
    inventory,
  )

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
          {worker ? (
            <WorkerNameControl
              defaultLabel={defaultLabel}
              onRename={onRenameWorker}
              worker={worker}
            />
          ) : (
            <h2>{label}</h2>
          )}
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
                    {bindingRecoveryRequired
                      ? bindingDetail
                      : 'Restart this agent and reconcile it without replacing ownership.'}
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

interface ProjectOrchestratorReplacementDetails {
  objective: string
  profileId: string
  role: string
}

function ProjectOrchestratorReplacementDialog({
  busy,
  error,
  onClose,
  onConfirm,
  profiles,
  project,
  returnFocus,
}: {
  busy: boolean
  error: string | null
  onClose: () => void
  onConfirm: (details: ProjectOrchestratorReplacementDetails) => Promise<void>
  profiles: WorkerProfile[]
  project: Project
  returnFocus: HTMLElement | null
}) {
  const dialogRef = useRef<HTMLElement>(null)
  const eligibleProfiles = profiles.filter(
    (profile) => profile.runtime_adapter === project.runtime.adapter,
  )
  const preferredProfile =
    eligibleProfiles.find((profile) => profile.default_role === 'orchestrator') ??
    eligibleProfiles[0]
  const [profileId, setProfileId] = useState(preferredProfile?.id ?? '')
  const [objective, setObjective] = useState(
    `Continue orchestration for ${project.name}.`,
  )
  const [role, setRole] = useState(
    preferredProfile?.default_role ?? 'orchestrator',
  )
  useModalDialog({
    canClose: !busy,
    dialogRef,
    onClose,
    returnFocus,
  })

  const submit = (event: FormEvent) => {
    event.preventDefault()
    void onConfirm({ objective, profileId, role })
  }

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        aria-labelledby="project-orchestrator-replacement-title"
        aria-modal="true"
        className="control-dialog orchestrator-transfer-dialog"
        ref={dialogRef}
        role="dialog"
      >
        <header className="dialog-heading">
          <div>
            <p className="eyebrow">Binding recovery</p>
            <h2 id="project-orchestrator-replacement-title">
              Replace orchestrator
            </h2>
          </div>
          <button
            aria-label="Close orchestrator replacement"
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
            <span>Replacement profile</span>
            <select
              autoFocus
              disabled={busy || eligibleProfiles.length === 0}
              onChange={(event) => {
                const nextProfile = eligibleProfiles.find(
                  (profile) => profile.id === event.target.value,
                )
                setProfileId(event.target.value)
                if (nextProfile) setRole(nextProfile.default_role)
              }}
              value={profileId}
            >
              {eligibleProfiles.map((profile) => (
                <option key={profile.id} value={profile.id}>
                  {profile.name} · v{profile.version}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>Objective</span>
            <textarea
              disabled={busy}
              maxLength={16000}
              onChange={(event) => setObjective(event.target.value)}
              required
              rows={4}
              value={objective}
            />
          </label>
          <label>
            <span>Role</span>
            <input
              disabled={busy}
              maxLength={240}
              onChange={(event) => setRole(event.target.value)}
              required
              value={role}
            />
          </label>
          <div className="end-session-impact">
            <CircleAlert aria-hidden="true" size={17} />
            <p>
              Yard starts and verifies a fresh worker before cutover. The
              current worker loses project ownership but remains available for
              inspection; replacement does not mark its work complete.
            </p>
          </div>
          {error ? (
            <p className="dialog-error" role="alert">
              <CircleAlert aria-hidden="true" size={16} />
              <span>{error}</span>
            </p>
          ) : null}
          <footer className="dialog-actions">
            <button
              className="secondary-button"
              disabled={busy}
              onClick={onClose}
              type="button"
            >
              Keep current worker
            </button>
            <button
              className="command-button"
              disabled={
                busy ||
                !profileId ||
                !objective.trim() ||
                !role.trim()
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
                <RefreshCw aria-hidden="true" size={16} />
              )}
              Replace orchestrator
            </button>
          </footer>
        </form>
      </section>
    </div>
  )
}

function ProjectOrchestratorInspector({
  busy,
  candidates,
  eligibilityReason,
  inventory,
  inventoryError,
  inventoryLoading,
  label,
  onChange,
  onCreateSummary,
  onReceiveSummary,
  onReplace,
  onRefresh,
  project,
  profiles,
  statusReport,
  summaries,
  summaryError,
}: {
  busy: boolean
  candidates: WorkerCandidate[]
  eligibilityReason: ProjectOrchestratorEligibilityReason
  inventory: RuntimeInventory | null
  inventoryError: string | null
  inventoryLoading: boolean
  label: string
  onChange: (trigger: HTMLButtonElement) => void
  onCreateSummary: (profile: WorkerProfile) => void
  onReceiveSummary: (summary: SummaryWorker) => void
  onReplace: (trigger: HTMLButtonElement) => void
  onRefresh: () => void
  project: Project
  profiles: WorkerProfile[]
  statusReport: StatusReport | undefined
  summaries: SummaryWorker[]
  summaryError: string | null
}) {
  const runtime = project.orchestrator.runtime
  const snapshotCurrent = Boolean(inventory && !inventoryError)
  const capabilities = resolveRuntimeCapabilities(
    snapshotCurrent,
    runtime,
    inventory,
  )
  const runtimeState = resolvedRuntimeState(runtime, capabilities)
  const bindingRecoveryRequired =
    capabilities.reason === 'binding_missing' ||
    capabilities.reason === 'identity_mismatch'
  const transferStatusId = `project-orchestrator-transfer-status-${project.id}`
  const compatibleProfiles = profiles.filter(
    (profile) => profile.runtime_adapter === project.runtime.adapter,
  )
  const [summaryProfileId, setSummaryProfileId] = useState(
    compatibleProfiles[0]?.id ?? '',
  )
  useEffect(() => {
    if (!compatibleProfiles.some((profile) => profile.id === summaryProfileId)) {
      setSummaryProfileId(compatibleProfiles[0]?.id ?? '')
    }
  }, [compatibleProfiles, summaryProfileId])
  const summaryProfile = compatibleProfiles.find(
    (profile) => profile.id === summaryProfileId,
  )

  return (
    <>
      <div className="inspector__identity">
        <span className="inspector__icon" data-status={runtimeState.status}>
          <BriefcaseBusiness aria-hidden="true" size={20} />
        </span>
        <div>
          <p className="eyebrow">Project orchestrator</p>
          <h2>{label}</h2>
          <span
            className="availability-badge"
            data-availability="orchestrator"
          >
            <BriefcaseBusiness aria-hidden="true" size={12} />
            Orchestrator
          </span>
        </div>
      </div>
      <RuntimeStateSummary
        compact
        inventory={inventory}
        runtime={runtime}
        snapshotCurrent={snapshotCurrent}
      />
      <div className="inspector-actions project-orchestrator-actions">
        {bindingRecoveryRequired ? (
          <button
            className="command-button project-orchestrator-replace"
            onClick={(event) => onReplace(event.currentTarget)}
            type="button"
          >
            <RefreshCw aria-hidden="true" size={15} />
            Replace orchestrator
          </button>
        ) : null}
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
      </div>
      {statusReport ? (
        <WorkflowStatusSummary report={statusReport} />
      ) : null}
      {!bindingRecoveryRequired ? (
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
      ) : null}
      <section aria-label="Ephemeral summary workers" className="ephemeral-summary-section">
        <p className="eyebrow">Ephemeral summary</p>
        <p className="project-orchestrator-transfer-status">
          This user-initiated action creates and prompts a short-lived worker,
          which spends provider tokens. It opens a background tab in this
          orchestrator&apos;s captured workspace.
        </p>
        <label>
          Worker profile
          <select
            disabled={busy || compatibleProfiles.length === 0}
            onChange={(event) => setSummaryProfileId(event.target.value)}
            value={summaryProfileId}
          >
            {compatibleProfiles.map((profile) => (
              <option key={profile.id} value={profile.id}>
                {profile.name}
              </option>
            ))}
          </select>
        </label>
        <button
          className="secondary-button"
          disabled={busy || !summaryProfile}
          onClick={() => summaryProfile && onCreateSummary(summaryProfile)}
          type="button"
        >
          {busy ? (
            <LoaderCircle aria-hidden="true" className="status-spin" size={15} />
          ) : (
            <Bot aria-hidden="true" size={15} />
          )}
          Create summary worker
        </button>
        {summaryError ? (
          <div className="dialog-error" role="alert">
            <CircleAlert aria-hidden="true" size={16} />
            <span>{summaryError}</span>
          </div>
        ) : null}
        {summaries.map((summary) => (
          <div className="project-orchestrator-transfer-status" key={summary.command_id}>
            <strong>{summary.state.replaceAll('_', ' ')}</strong>
            {summary.retirement_reason ? ` — ${summary.retirement_reason}` : null}
            {summary.error ? ` — ${summary.error}` : null}
            {summary.state === 'ready' ? (
              <button
                className="secondary-button"
                disabled={busy}
                onClick={() => onReceiveSummary(summary)}
                type="button"
              >
                Receive summary
              </button>
            ) : null}
          </div>
        ))}
      </section>
      <details className="worker-inspector-details">
        <summary>Details</summary>
        <dl className="detail-list">
          <DetailRow label="Worker ID" value={project.orchestrator.id} mono />
          <DetailRow label="Herdr session" value={runtime?.session} mono />
          <DetailRow label="Terminal" value={runtime?.terminal_id} mono />
        </dl>
      </details>
    </>
  )
}

function ProjectInspector({
  accent,
  inventory,
  orchestratorLabel,
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
  orchestratorLabel: string
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
      <ProjectRepositoriesSection
        onChanged={onRefresh}
        project={project}
        suggestedRoot={workspace?.worktree?.checkout_path}
      />
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
            value={orchestratorLabel}
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

/** Everything an inspector needs to complete or end an active assignment. */
interface AssignmentDispositionControls {
  busy: boolean
  completeAndEndSession: boolean
  error: string | null
  onComplete: () => void
  onCompleteAndEndSessionChange: (enabled: boolean) => void
  onCompleteWithDetails: () => void
  onDelete: (trigger: HTMLElement | null) => void
  onEndSession: (trigger: HTMLElement | null) => void
  onRetry?: () => void
  onUndo: () => void
  pending: boolean
}

function ActiveAssignmentActions({
  assignment,
  controls,
}: {
  assignment: Assignment
  controls: AssignmentDispositionControls
}) {
  return (
    <AssignmentActions
      assignment={assignment}
      busy={controls.busy}
      completeAndEndSession={controls.completeAndEndSession}
      error={controls.error}
      onComplete={controls.onComplete}
      onCompleteAndEndSessionChange={controls.onCompleteAndEndSessionChange}
      onCompleteWithDetails={controls.onCompleteWithDetails}
      onDelete={() =>
        controls.onDelete(
          document.activeElement instanceof HTMLElement
            ? document.activeElement
            : null,
        )
      }
      onEndSession={() =>
        controls.onEndSession(
          document.activeElement instanceof HTMLElement
            ? document.activeElement
            : null,
        )
      }
      onRetry={controls.onRetry}
      onUndo={controls.onUndo}
      pending={controls.pending}
    />
  )
}

const CANCELLATION_REASON_LABELS: Record<
  AssignmentCancellation['reason'],
  string
> = {
  ended_without_completion: 'Ended without completion',
  project_archived: 'Project archived',
}

function AssignmentInspector({
  assignment,
  controls,
  inventory,
  defaultLabel,
  onDelete,
  onEndSession,
  onRefresh,
  snapshotCurrent,
  onRenameWorker,
}: {
  assignment: Assignment
  controls: AssignmentDispositionControls
  inventory: RuntimeInventory | null
  // The label without the user-chosen name (rename field placeholder).
  defaultLabel: string
  onDelete?: () => void
  onEndSession?: () => void
  onRefresh: () => void
  snapshotCurrent: boolean
  onRenameWorker?: RenameWorkerHandler
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
  const cancellation = assignment.cancellation

  return (
    <>
      <div className="inspector__identity">
        <span className="inspector__icon" data-status={status}>
          <Bot aria-hidden="true" size={20} />
        </span>
        <div>
          <p className="eyebrow">Assignment</p>
          <WorkerNameControl
            defaultLabel={defaultLabel}
            onRename={onRenameWorker}
            worker={assignment.worker}
          />
        </div>
      </div>
      <span className="runtime-badge" data-lifecycle={assignment.lifecycle}>
        {assignment.lifecycle === 'cancelled'
          ? 'ended without completion'
          : assignment.lifecycle}
      </span>
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
          assignment.lifecycle,
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
            <strong>Ready to complete</strong>
            <small>
              Agent reports done. Complete it, or add details to the receipt.
            </small>
          </span>
        </div>
      ) : null}
      {receipt ? (
        <section
          aria-label="Completion receipt"
          className="completion-receipt"
          data-detail-level={receipt.detail_level}
        >
          <p className="eyebrow">Completion receipt</p>
          <dl className="detail-list">
            <DetailRow label="Summary" value={receipt.summary} />
            <DetailRow label="Outcome" value={receipt.outcome} />
            <DetailRow
              label="Detail"
              value={
                receipt.detail_level === 'minimal'
                  ? 'Minimal — no detailed handoff'
                  : 'Detailed'
              }
            />
            {receipt.objective_snapshot ? (
              <DetailRow
                label="Objective at completion"
                value={receipt.objective_snapshot}
              />
            ) : null}
            {receipt.detail_level === 'detailed' ? (
              <>
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
              </>
            ) : null}
            <DetailRow label="Actor" value={receipt.actor} mono />
            <DetailRow
              label="Recorded"
              value={new Date(receipt.created_at_unix_ms).toISOString()}
              mono
            />
          </dl>
        </section>
      ) : null}
      {cancellation ? (
        <section
          aria-label="Cancellation"
          className="completion-receipt assignment-cancellation"
        >
          <p className="eyebrow">Not completed</p>
          <dl className="detail-list">
            <DetailRow
              label="Outcome"
              value={CANCELLATION_REASON_LABELS[cancellation.reason]}
            />
            <DetailRow
              label="Objective"
              value={cancellation.objective_snapshot}
            />
            <DetailRow label="Actor" value={cancellation.actor} mono />
            <DetailRow
              label="Recorded"
              value={new Date(cancellation.cancelled_at_unix_ms).toISOString()}
              mono
            />
          </dl>
        </section>
      ) : null}
      {(receipt || cancellation) && (onEndSession || onDelete) ? (
        <div className="inspector-actions disposition-actions">
          <div className="awaiting-disposition" role="status">
            <CircleCheck aria-hidden="true" size={16} />
            <span>
              <strong>Awaiting disposition</strong>
              <small>
                {receipt
                  ? 'The completion receipt is retained whether this session is reassigned or ended.'
                  : 'The cancellation is retained whether this session is reassigned or ended.'}
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
        <ActiveAssignmentActions assignment={assignment} controls={controls} />
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
  workerNames = {},
  workers,
  workspace,
}: {
  busy: boolean
  onCreate: (details: ProjectCreationDetails) => Promise<void>
  profiles: WorkerProfile[]
  workerNames?: Record<string, string>
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
                  {workerLabelWithDefault(
                    {
                      displayName: workerNames[worker.runtime_id],
                      observedDisplayProvider: worker.display_provider,
                      observedName: worker.name,
                      observedProvider: worker.provider,
                      workerId: worker.runtime_id,
                      workspaceLabel: workspace.label,
                    },
                    workers.map((candidate) => candidate.runtime_id),
                  )}
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
  const [settingsInitialFocus, setSettingsInitialFocus] = useState<
    'appearance' | 'automatic'
  >('appearance')
  const [herdrInventoryOpen, setHerdrInventoryOpen] = useState(false)
  const [sessions, setSessions] = useState<RuntimeSession[]>([])
  const [selectedSession, setSelectedSession] = useState('')
  const [projectsResolved, setProjectsResolved] = useState(false)
  const explicitSessionRef = useRef<string | null>(readStoredSession())
  const sessionSelectionSettledRef = useRef(false)
  const [inventory, setInventory] = useState<RuntimeInventory | null>(null)
  const [inventoryCurrent, setInventoryCurrent] = useState(false)
  const [lensEntries, setLensEntries] = useState<RuntimeLensEntry[]>([])
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
  const [summaryWorkers, setSummaryWorkers] = useState<SummaryWorker[]>([])
  const [summaryWorkerBusy, setSummaryWorkerBusy] = useState(false)
  const [summaryWorkerError, setSummaryWorkerError] = useState<string | null>(
    null,
  )
  const [projectStatusReports, setProjectStatusReports] =
    useState<ProjectStatusReports>({})
  const [projectPulseOpen, setProjectPulseOpen] = useState(false)
  const [filter, setFilter] = useState<Filter>('current')
  const [railView, setRailView] = useState<ResourceView>('profiles')
  const [resourceShelfOpen, setResourceShelfOpen] = useState(false)
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
  const [projectOrchestratorReplacement, setProjectOrchestratorReplacement] =
    useState<{
      commandId: string
      projectId: string
      returnFocus: HTMLElement | null
    } | null>(null)
  const [
    projectOrchestratorReplacementBusy,
    setProjectOrchestratorReplacementBusy,
  ] = useState(false)
  const [
    projectOrchestratorReplacementError,
    setProjectOrchestratorReplacementError,
  ] = useState<string | null>(null)
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
  const [completedRuntimeCleanupPreview, setCompletedRuntimeCleanupPreview] =
    useState<CompletedRuntimeCleanupPreview | null>(null)
  const [
    completedRuntimeCleanupPreviewOpen,
    setCompletedRuntimeCleanupPreviewOpen,
  ] = useState(false)
  const [
    completedRuntimeCleanupPreviewLoading,
    setCompletedRuntimeCleanupPreviewLoading,
  ] = useState(false)
  const [
    completedRuntimeCleanupPreviewError,
    setCompletedRuntimeCleanupPreviewError,
  ] = useState<string | null>(null)
  const completedRuntimeCleanupPreviewTrigger = useRef<HTMLButtonElement>(null)
  const [workerDeleteProposal, setWorkerDeleteProposal] = useState<{
    candidate: WorkerCandidate
    deleteCommandId: string
    endCommandId: string
    hideOnly: boolean
  } | null>(null)
  const [workerDeleteBusy, setWorkerDeleteBusy] = useState(false)
  const [workerDeleteError, setWorkerDeleteError] = useState<string | null>(
    null,
  )
  const [staleHideProposal, setStaleHideProposal] = useState<{
    items: Array<{
      candidate: WorkerCandidate
      deleteCommandId: string
      endCommandId: string
    }>
    returnFocus: HTMLElement | null
  } | null>(null)
  const [staleHideBusy, setStaleHideBusy] = useState(false)
  const [staleHideError, setStaleHideError] = useState<string | null>(null)
  // One-click Complete waits QUICK_COMPLETE_UNDO_MS before it is sent, so
  // Undo simply never sends it. Nothing is durable during the window.
  const [completeAndEndSession, setCompleteAndEndSession] = useState(
    readCompleteAndEndSession,
  )
  const [dispositionBusy, setDispositionBusy] = useState<
    Record<string, boolean>
  >({})
  const [dispositionErrors, setDispositionErrors] = useState<
    Record<string, { endSession: boolean; message: string; outcome: DispositionOutcome }>
  >({})
  // A disposition command ID is created when the action starts and reused
  // on every retry of the identical request until it settles.
  const dispositionCommands = useRef(new DispositionCommandIds())
  const [workerDispositionProposal, setWorkerDispositionProposal] = useState<{
    assignment: Assignment
    deleteCommandId: string
    mode: WorkerDispositionMode
    returnFocus: HTMLElement | null
  } | null>(null)
  const [workerDispositionBusy, setWorkerDispositionBusy] =
    useState<WorkerDispositionChoice | null>(null)
  const [workerDispositionError, setWorkerDispositionError] = useState<
    string | null
  >(null)
  // One confirmation per project archive or delete. The command ID belongs
  // to one preview and is reused on every retry of it, so a retry sends the
  // identical body; a refreshed preview gets a new command ID.
  const [projectArchiveProposal, setProjectArchiveProposal] =
    useState<ProjectDispositionProposal | null>(null)
  const [projectArchiveBusy, setProjectArchiveBusy] = useState(false)
  const [projectArchiveError, setProjectArchiveError] = useState<
    string | null
  >(null)
  const [projectDeleteProposal, setProjectDeleteProposal] =
    useState<ProjectDispositionProposal | null>(null)
  const [projectDeleteBusy, setProjectDeleteBusy] = useState(false)
  const [projectDeleteError, setProjectDeleteError] = useState<string | null>(
    null,
  )
  // One sheet per workstream archive or delete. The command ID is created
  // with each preview and reused on every retry of that preview, so a retry
  // sends the identical body. A version conflict commits nothing, so the
  // sheet then loads a fresh preview under a new command ID.
  const [workstreamDispositionProposal, setWorkstreamDispositionProposal] =
    useState<{
      commandId: string
      mode: 'archive' | 'delete'
      node: CoordinationNode
      preview: CoordinationNodeDispositionPreview | null
      previewError: string | null
      returnFocus: HTMLButtonElement | null
    } | null>(null)
  const [workstreamDispositionBusy, setWorkstreamDispositionBusy] =
    useState(false)
  const [workstreamDispositionError, setWorkstreamDispositionError] = useState<
    string | null
  >(null)
  const [runtimeError, setRuntimeError] = useState<string | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)
  const [actionNotice, setActionNotice] = useState<string | null>(null)
  // Archived (not deleted) projects for the Archived shelf; null until read.
  const [archivedProjects, setArchivedProjects] = useState<
    ArchivedProjectSummary[] | null
  >(null)
  const [archivedProjectsError, setArchivedProjectsError] = useState<
    string | null
  >(null)
  // Restore state per archive command. The restore command ID is created
  // with the first attempt and reused on every retry until it succeeds.
  const [projectRestores, setProjectRestores] = useState<
    Record<string, ProjectRestoreState>
  >({})
  const restoreCommandIds = useRef<Record<string, string>>({})
  // The Undo offered for ARCHIVE_UNDO_WINDOW_MS after a restorable archive.
  const [archiveUndo, setArchiveUndo] = useState<{
    archiveCommandId: string
    projectId: string
    projectName: string
    shownAt: number
  } | null>(null)
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
  const herdrInventoryTrigger = useRef<HTMLButtonElement | null>(null)
  const resourceShelfTrigger = useRef<HTMLButtonElement | null>(null)
  const resourceShelf = useRef<HTMLElement | null>(null)
  const settingsTrigger = useRef<HTMLButtonElement | null>(null)
  const agentWorkspaceReturnFocus = useRef<HTMLElement | null>(null)


  useEffect(() => {
    applyTheme(theme)
  }, [theme])

  useEffect(() => {
    writeMapVisualMode(mapVisualMode)
  }, [mapVisualMode])

  useEffect(() => {
    if (!resourceShelfOpen || !resourceShelfTrigger.current) return
    const frame = window.requestAnimationFrame(() =>
      resourceShelf.current
        ?.querySelector<HTMLButtonElement>(
          '.resource-shelf__tabs [aria-selected="true"]',
        )
        ?.focus(),
    )
    return () => window.cancelAnimationFrame(frame)
  }, [resourceShelfOpen])

  useEffect(() => {
    projectsRef.current = projects
  }, [projects])

  useEffect(() => {
    coordinationNodesRef.current = coordinationNodes
  }, [coordinationNodes])

  useEffect(() => {
    automationsRef.current = automations
  }, [automations])

  // Set once the first session discovery settles, so an empty selection
  // during discovery keeps Herdr in its loading state.
  const sessionDiscoverySettledRef = useRef(false)

  const loadSessions = useCallback(async (signal?: AbortSignal) => {
    const result = await fetchSessions(signal)
    setSessions(result.sessions)
    return result.sessions
  }, [])

  const sessionRoleCounts = useMemo(
    () => countSessionRoles(projects, coordinationNodes),
    [coordinationNodes, projects],
  )

  useEffect(() => {
    const explicit = explicitSessionRef.current
    const explicitRunning = Boolean(
      explicit &&
        sessions.some(
          (session) => session.name === explicit && session.running,
        ),
    )
    // Wait for project data before an automatic choice so the selection does
    // not start on Herdr's default session and then jump. A stored choice
    // only skips the wait while that session is running.
    if (!projectsResolved && !explicitRunning) return
    // Read the ref now: React may run the updater later, after the ref below
    // has already been flipped.
    const settled = sessionSelectionSettledRef.current
    setSelectedSession((current) =>
      resolveSelectedSession({
        counts: sessionRoleCounts,
        current,
        explicit,
        sessions,
        settled,
      }),
    )
    if (projectsResolved && sessions.some((session) => session.running)) {
      sessionSelectionSettledRef.current = true
    }
  }, [projectsResolved, sessionRoleCounts, sessions])

  const chooseSession = useCallback((session: string) => {
    explicitSessionRef.current = session || null
    writeStoredSession(session)
    setSelectedSession(session)
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

  const loadSummaryWorkers = useCallback(
    async (projectIds: string[], signal?: AbortSignal) => {
      const results = await Promise.all(
        projectIds.map((projectId) => fetchSummaryWorkers(projectId, signal)),
      )
      const summaries = results.flatMap((result) => result.summaries)
      setSummaryWorkers(summaries)
      return summaries
    },
    [],
  )

  const loadProjects = useCallback(
    async (signal?: AbortSignal) => {
      const result = await fetchProjects(signal)
      setProjects(result.projects)
      setProjectsResolved(true)
      const projectIds = result.projects.map((project) => project.id)
      const [loadedAssignments] = await Promise.all([
        loadAssignments(projectIds, signal),
        loadSummaryWorkers(projectIds, signal),
      ])
      return loadedAssignments
    },
    [loadAssignments, loadSummaryWorkers],
  )

  const loadArchivedProjects = useCallback(async (signal?: AbortSignal) => {
    try {
      const result = await fetchArchivedProjects(signal)
      setArchivedProjects(result.projects)
      setArchivedProjectsError(null)
    } catch (caught) {
      if (signal?.aborted) return
      setArchivedProjectsError(
        caught instanceof Error
          ? caught.message
          : 'Archived projects could not be loaded',
      )
    }
  }, [])

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

  // A rename changes only the label: every copy of the worker Yard holds
  // (candidates, assignments, orchestrators, workstreams) takes the server's
  // answer so each view shows the same name.
  const applyRenamedWorker = useCallback((renamed: NamedWorker & { id: string }) => {
    const patch = <T extends Worker>(worker: T): T =>
      withRenamedDisplayName(worker, renamed)
    setWorkerCandidates((current) =>
      current.map((candidate) =>
        candidate.worker.id === renamed.id
          ? { ...candidate, worker: patch(candidate.worker) }
          : candidate,
      ),
    )
    setAssignments((current) =>
      current.map((assignment) =>
        assignment.worker.id === renamed.id
          ? { ...assignment, worker: patch(assignment.worker) }
          : assignment,
      ),
    )
    setProjects((current) =>
      current.map((project) =>
        project.orchestrator.id === renamed.id
          ? { ...project, orchestrator: patch(project.orchestrator) }
          : project,
      ),
    )
    setYardOrchestrator((current) =>
      current?.worker?.id === renamed.id
        ? { ...current, worker: patch(current.worker) }
        : current,
    )
    setCoordinationNodes((current) =>
      current.map((node) =>
        node.worker?.id === renamed.id
          ? { ...node, worker: patch(node.worker) }
          : node,
      ),
    )
  }, [])

  const renameWorkerLabel = useCallback<RenameWorkerHandler>(
    async (worker, displayName, expectedDisplayName) => {
      try {
        const result = await renameWorker(worker.id, {
          actor: 'local-user',
          command_id: crypto.randomUUID(),
          display_name: displayName,
          expected_display_name: expectedDisplayName,
        })
        applyRenamedWorker(result.worker)
        return result.worker
      } catch (caught) {
        if (
          caught instanceof YardApiError &&
          caught.code === 'worker_name_conflict'
        ) {
          // Show the name someone else chose rather than the stale one,
          // in every view, so a retry sends the current name.
          if (caught.currentDisplayName !== undefined) {
            applyRenamedWorker({
              display_name: caught.currentDisplayName,
              id: worker.id,
            })
          }
          void loadWorkers().catch(() => undefined)
        }
        throw caught
      }
    },
    [applyRenamedWorker, loadWorkers],
  )

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
        setLensEntries([])
        if (!background && sessionDiscoverySettledRef.current) {
          setRuntimeLoading(false)
        }
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
        setLensEntries([])
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
        const lens = await fetchRuntimeLens(session, signal)
        setInventory((current) =>
          reconcileInventorySnapshot(current, lens.inventory),
        )
        setRuntimeTopology(lens.topology)
        setWorkerCandidates((current) =>
          reconcileRuntimeProjectionSnapshot(current, lens.workers.workers),
        )
        setLensEntries(lens.entries)
        setInventoryCurrent(lens.snapshot_current)
        setRuntimeError(null)
        try {
          await loadYardOrchestrator(signal)
        } catch (caught) {
          if (caught instanceof DOMException && caught.name === 'AbortError') {
            return
          }
          setRuntimeError(
            caught instanceof Error
              ? caught.message
              : 'Yard orchestrator refresh failed',
          )
        }
      } catch (caught) {
        if (caught instanceof DOMException && caught.name === 'AbortError') {
          return
        }
        setInventoryCurrent(false)
        setRuntimeError(
          caught instanceof Error
            ? caught.message
            : 'Runtime lens request failed',
        )
      } finally {
        if (!background && !signal?.aborted) setRuntimeLoading(false)
      }
    },
    [loadYardOrchestrator],
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
        sessionDiscoverySettledRef.current = true
        if (!availableSessions.some((session) => session.running)) {
          setRuntimeLoading(false)
        }
      })
      .catch((caught: unknown) => {
        if (caught instanceof DOMException && caught.name === 'AbortError') {
          return
        }
        sessionDiscoverySettledRef.current = true
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
      loadProjects(controller.signal).catch((caught: unknown) => {
        // Let the automatic Herdr session choice proceed without projects.
        if (!controller.signal.aborted) setProjectsResolved(true)
        throw caught
      }),
      loadYardOrchestrator(controller.signal),
      loadCoordination(controller.signal),
      loadAutomations(controller.signal),
      loadTokenSpendSettings(controller.signal),
      loadOrchestratorWorkflowProfile(controller.signal),
      loadProfiles(controller.signal),
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
        await loadSummaryWorkers(
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
  }, [loadAssignments, loadSummaryWorkers])

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

  // Each durable worker's label (its user-chosen name first) and the label
  // it would have without a name (shown beside the name and in the rename
  // field), both through the one `workerDisplay` rule.
  const [workerLabels, workerDefaultLabels, workerListLabels] = useMemo(() => {
    const peerWorkerIds = workerCandidates.map(({ worker }) => worker.id)
    const sources = workerCandidates.map(
      (candidate) =>
        [
          candidate.worker.id,
          workerLabelSource(
            candidate,
            assignments,
            projects,
            inventory,
            inventoryCurrent,
          ),
        ] as const,
    )
    return [
      Object.fromEntries(
        sources.map(([id, source]) => [
          id,
          workerDisplayLabel(source, peerWorkerIds),
        ]),
      ),
      Object.fromEntries(
        sources.map(([id, source]) => [
          id,
          workerDefaultLabel(source, peerWorkerIds),
        ]),
      ),
      // "BAR CDK · Generalist" in the worker list, where a row has room.
      Object.fromEntries(
        sources.map(([id, source]) => [
          id,
          workerLabelWithDefault(source, peerWorkerIds),
        ]),
      ),
    ] as [
      Record<string, string>,
      Record<string, string>,
      Record<string, string>,
    ]
  }, [
    assignments,
    inventory,
    inventoryCurrent,
    projects,
    workerCandidates,
  ])
  // The lifecycle of each worker's most recent assignment: work ended
  // without completion keeps its worker quiet instead of stale.
  const latestAssignmentLifecycles = useMemo(() => {
    const latest = new Map<string, Assignment>()
    for (const assignment of assignments) {
      const current = latest.get(assignment.worker.id)
      if (
        !current ||
        assignment.created_at_unix_ms > current.created_at_unix_ms
      ) {
        latest.set(assignment.worker.id, assignment)
      }
    }
    return new Map(
      [...latest].map(([workerId, assignment]) => [
        workerId,
        assignment.lifecycle,
      ]),
    )
  }, [assignments])
  const visibleCandidates = useMemo(
    () =>
      workerCandidates.filter((candidate) =>
        matchesFilter(
          candidate,
          filter,
          inventoryCurrent,
          inventory,
          latestAssignmentLifecycles.get(candidate.worker.id) ?? null,
        ),
      ),
    [
      filter,
      inventory,
      inventoryCurrent,
      latestAssignmentLifecycles,
      workerCandidates,
    ],
  )
  const attentionCount = useMemo(
    () =>
      workerCandidates.filter((candidate) =>
        matchesFilter(
          candidate,
          'attention',
          inventoryCurrent,
          inventory,
          latestAssignmentLifecycles.get(candidate.worker.id) ?? null,
        ),
      ).length,
    [inventory, inventoryCurrent, latestAssignmentLifecycles, workerCandidates],
  )
  const staleCandidates = useMemo(
    () =>
      workerCandidates.filter((candidate) =>
        matchesFilter(
          candidate,
          'stale',
          inventoryCurrent,
          inventory,
          latestAssignmentLifecycles.get(candidate.worker.id) ?? null,
        ),
      ),
    [inventory, inventoryCurrent, latestAssignmentLifecycles, workerCandidates],
  )
  const hideableStaleCandidates = useMemo(
    () =>
      staleCandidates.filter(
        (candidate) =>
          candidate.worker.ownership_kind === 'external' &&
          !workerCurrentlyObserved(candidate, inventoryCurrent, inventory) &&
          canDeleteCandidate(candidate),
      ),
    [inventory, inventoryCurrent, staleCandidates],
  )
  const automaticCoordination = tokenSpendSettings
    ? [
        tokenSpendSettings.superintendent_auto_requests_project_summaries
          ? 'project summaries'
          : null,
        tokenSpendSettings.project_orchestrators_auto_request_worker_summaries
          ? 'worker summaries'
          : null,
        tokenSpendSettings.scheduled_automatic_summaries
          ? 'scheduled summaries'
          : null,
      ].filter((label): label is string => label !== null)
    : []
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
  const workerDisplayNameByRuntimeId = useMemo(
    () =>
      Object.fromEntries(
        (inventory?.workers ?? []).flatMap((worker) => {
          const candidate = findCandidateForObservedWorker(
            workerCandidates,
            inventory,
            worker,
          )
          const name = workerDisplayName(candidate?.worker)
          return name ? [[worker.runtime_id, name]] : []
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
  const selectedWorkerAttentionState = selectedWorkerCandidate
    ? workerAttentionState(
        selectedWorkerCandidate,
        inventoryCurrent,
        inventory,
        latestAssignmentLifecycles.get(selectedWorkerCandidate.worker.id) ??
          null,
      )
    : 'quiet'
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
  const projectOrchestratorReplacementProject =
    projectOrchestratorReplacement
      ? projects.find(
          (project) =>
            project.id === projectOrchestratorReplacement.projectId,
        )
      : undefined
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
            label:
              workerLabels[assignment.worker.id] ??
              workerDisplayLabel({
               displayName: assignment.worker.display_name,
                assignmentRole: assignment.role,
                profileName: assignment.profile_name,
                projectName: project?.name,
                workerId: assignment.worker.id,
              }),
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
          label:
            workerLabels[project.orchestrator.id] ??
            workerDisplayLabel({
             displayName: project.orchestrator.display_name,
              projectOrchestratorName: project.name,
              workerId: project.orchestrator.id,
            }),
          project,
          status: runtimeCapabilityStatus(
            project.orchestrator.runtime,
            capabilities,
          ),
        },
      ]
    })
  }, [
    assignments,
    inventory,
    inventoryCurrent,
    projects,
    selection,
    workerLabels,
  ])
  const selectedCandidateCompletion = selectedWorkerCandidate
    ? assignments
        .filter(
          (assignment) =>
            assignment.worker.id === selectedWorkerCandidate.worker.id &&
            (assignment.lifecycle === 'completed' ||
              assignment.lifecycle === 'cancelled'),
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
          null,
          runtimeBindingReason(inventoryCurrent, yardRuntime, inventory) ?? null,
        ),
        contextLabel: 'Yard portfolio',
        key: 'yard-orchestrator',
        label:
          workerLabels[yardWorker.id] ??
          workerDisplayLabel({
           displayName: yardWorker.display_name,
            projectOrchestratorName: 'Yard',
            workerId: yardWorker.id,
          }),
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
          runtimeBindingReason(inventoryCurrent, runtime, inventory) ?? null,
        ),
        contextLabel:
          attachedProjects.join(', ') || 'Yard workstream',
        key: `coordination-node:${node.id}`,
        label: labelWithDisplayName(worker.display_name, node.name),
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
          null,
          runtimeBindingReason(inventoryCurrent, runtime, inventory) ?? null,
        ),
        contextLabel: project.name,
        key: `orchestrator:${project.id}`,
        label:
          workerLabels[project.orchestrator.id] ??
          workerDisplayLabel({
           displayName: project.orchestrator.display_name,
            projectOrchestratorName: project.name,
            workerId: project.orchestrator.id,
          }),
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
          null,
          runtimeBindingReason(inventoryCurrent, runtime, inventory) ?? null,
        ),
        contextLabel:
          projectNames.get(assignment.project_id) ?? 'Yard project',
        key: `assignment:${assignment.id}`,
        label:
          workerLabels[assignment.worker.id] ??
          workerDisplayLabel({
           displayName: assignment.worker.display_name,
            assignmentRole: assignment.role,
            profileName: assignment.profile_name,
            projectName: projectNames.get(assignment.project_id),
            workerId: assignment.worker.id,
          }),
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
    workerLabels,
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
    : null
  const openAgentChat = useCallback((target: AgentWorkspaceTarget) => {
    if (!target.chatAvailable) return
    agentWorkspaceReturnFocus.current = target.returnFocus ?? null
    setAgentWorkspaceTarget(target)
    setAgentWorkspaceMode('chat')
  }, [])
  const openAgentTerminal = useCallback(
    (target: AgentWorkspaceTarget) => {
      if (!target.interactive) return
      agentWorkspaceReturnFocus.current = target.returnFocus ?? null
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
      const refreshedSession = resolveSelectedSession({
        counts: sessionRoleCounts,
        current: selectedSession,
        explicit: explicitSessionRef.current,
        sessions: refreshedSessions,
        settled: true,
      })
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
    sessionRoleCounts,
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

  const proposeProjectOrchestratorReplacement = useCallback(
    (project: Project, returnFocus: HTMLElement) => {
      setActionError(null)
      setActionNotice(null)
      setProjectOrchestratorReplacementError(null)
      setProjectOrchestratorReplacement({
        commandId: crypto.randomUUID(),
        projectId: project.id,
        returnFocus,
      })
    },
    [],
  )

  const replaceSelectedProjectOrchestrator = useCallback(
    async ({
      objective,
      profileId,
      role,
    }: ProjectOrchestratorReplacementDetails) => {
      if (
        !projectOrchestratorReplacement ||
        !projectOrchestratorReplacementProject
      ) {
        return
      }
      const project = projectOrchestratorReplacementProject
      const runtime = project.orchestrator.runtime
      const profile = profiles.find((candidate) => candidate.id === profileId)
      if (!runtime || !profile) {
        setProjectOrchestratorReplacementError(
          'The current runtime or replacement profile is no longer available.',
        )
        return
      }

      setProjectOrchestratorReplacementBusy(true)
      setProjectOrchestratorReplacementError(null)
      try {
        const result = await replaceProjectOrchestrator(project.id, {
          command_id: projectOrchestratorReplacement.commandId,
          actor: 'local-user',
          expected_project_version: project.version,
          expected_orchestrator_worker_id: project.orchestrator.id,
          expected_orchestrator_worker_version:
            project.orchestrator.version,
          expected_orchestrator_runtime: runtime,
          profile_id: profile.id,
          expected_profile_version: profile.version,
          objective,
          role,
          old_session_disposition: 'retain_for_inspection',
          handoff_artifact_ref: null,
        })
        setProjects((current) =>
          current.map((candidate) =>
            candidate.id === result.project.id
              ? result.project
              : candidate,
          ),
        )
        setProjectOrchestratorReplacement(null)
        setActionNotice(
          result.cleanup_pending
            ? 'Orchestrator replaced. Previous runtime cleanup is pending.'
            : 'Orchestrator replaced. The previous worker remains available for inspection.',
        )
        await Promise.all([
          loadAssignments(projects.map(({ id }) => id)),
          loadInventory(selectedSession),
          loadProjects(),
          loadWorkers(),
        ])
      } catch (caught) {
        setProjectOrchestratorReplacementError(
          caught instanceof Error
            ? caught.message
            : 'Orchestrator replacement failed',
        )
      } finally {
        setProjectOrchestratorReplacementBusy(false)
      }
    },
    [
      loadAssignments,
      loadInventory,
      loadProjects,
      loadWorkers,
      profiles,
      projectOrchestratorReplacement,
      projectOrchestratorReplacementProject,
      projects,
      selectedSession,
    ],
  )

  const openCompletedRuntimeCleanupPreview = useCallback(async () => {
    setCompletedRuntimeCleanupPreviewOpen(true)
    setCompletedRuntimeCleanupPreviewLoading(true)
    setCompletedRuntimeCleanupPreviewError(null)
    try {
      setCompletedRuntimeCleanupPreview(
        await fetchCompletedRuntimeCleanupPreview(),
      )
    } catch (caught) {
      setCompletedRuntimeCleanupPreviewError(
        caught instanceof Error
          ? caught.message
          : 'Unable to load cleanup preview',
      )
    } finally {
      setCompletedRuntimeCleanupPreviewLoading(false)
    }
  }, [])

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

  const proposeWorkerDelete = useCallback(
    (candidate: WorkerCandidate, hideOnly = false) => {
      if (!canDeleteCandidate(candidate)) return
      setWorkerDeleteError(null)
      setActionNotice(null)
      setWorkerDeleteProposal({
        candidate,
        deleteCommandId: crypto.randomUUID(),
        endCommandId: crypto.randomUUID(),
        hideOnly,
      })
    },
    [],
  )

  const deleteSelectedWorker = useCallback(async () => {
    if (!workerDeleteProposal) return
    const { candidate, deleteCommandId, endCommandId } =
      workerDeleteProposal
    setWorkerDeleteBusy(true)
    setWorkerDeleteError(null)
    setActionError(null)
    try {
      const result = await hideWorkerCandidate(
        candidate,
        endCommandId,
        deleteCommandId,
      )
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
          ? 'Worker hidden from Yard. Runtime cleanup continues in the background.'
          : 'Worker hidden from Yard.',
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
          'Worker hidden from Yard. Runtime cleanup continues in the background if needed.',
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

  const proposeHideStaleWorkers = useCallback(
    (returnFocus: HTMLElement) => {
      const candidates = hideableStaleCandidates.slice(
        0,
        MAX_STALE_HIDE_BATCH,
      )
      if (candidates.length === 0) return
      setStaleHideError(null)
      setActionNotice(null)
      setStaleHideProposal({
        items: candidates.map((candidate) => ({
          candidate,
          deleteCommandId: crypto.randomUUID(),
          endCommandId: crypto.randomUUID(),
        })),
        returnFocus,
      })
    },
    [hideableStaleCandidates],
  )

  const hideStaleWorkers = useCallback(async () => {
    if (!staleHideProposal) return
    const currentCandidates = new Map(
      hideableStaleCandidates.map((candidate) => [
        candidate.worker.id,
        candidate,
      ]),
    )
    if (
      staleHideProposal.items.some(
        ({ candidate }) => !currentCandidates.has(candidate.worker.id),
      )
    ) {
      setStaleHideError(
        'The current Herdr snapshot changed. Close this dialog and review the stale workers again.',
      )
      return
    }

    setStaleHideBusy(true)
    setStaleHideError(null)
    setActionError(null)
    const completedIds = new Set<string>()
    for (const item of staleHideProposal.items) {
      const candidate = currentCandidates.get(item.candidate.worker.id)
      if (!candidate) continue
      try {
        await hideWorkerCandidate(
          candidate,
          item.endCommandId,
          item.deleteCommandId,
        )
        completedIds.add(candidate.worker.id)
      } catch (caught) {
        setStaleHideProposal((current) =>
          current
            ? {
                ...current,
                items: current.items.filter(
                  ({ candidate: remaining }) =>
                    !completedIds.has(remaining.worker.id),
                ),
              }
            : current,
        )
        setStaleHideError(
          `${completedIds.size} workers were hidden before Yard stopped: ${
            caught instanceof Error ? caught.message : 'Hide stale failed'
          }`,
        )
        await Promise.all([
          loadProjects(),
          loadWorkers(),
          loadInventory(selectedSession),
        ]).catch(() => undefined)
        setStaleHideBusy(false)
        return
      }
    }

    const hiddenCount = completedIds.size
    setWorkerCandidates((current) =>
      current.filter(({ worker }) => !completedIds.has(worker.id)),
    )
    setSelection((current) =>
      current?.kind === 'worker' && completedIds.has(current.id)
        ? null
        : current,
    )
    setStaleHideProposal(null)
    setActionNotice(
      `${hiddenCount} stale ${hiddenCount === 1 ? 'worker' : 'workers'} hidden from Yard.`,
    )
    await Promise.all([
      loadProjects(),
      loadWorkers(),
      loadInventory(selectedSession),
    ]).catch(() => undefined)
    setStaleHideBusy(false)
  }, [
    hideableStaleCandidates,
    loadInventory,
    loadProjects,
    loadWorkers,
    selectedSession,
    staleHideProposal,
  ])
  useEffect(() => {
    writeCompleteAndEndSession(completeAndEndSession)
  }, [completeAndEndSession])

  const reloadAssignment = useCallback(
    async (assignmentId: string) => {
      const loaded = await loadProjects()
      return loaded.find((assignment) => assignment.id === assignmentId)
    },
    [loadProjects],
  )

  const runDisposition = useCallback(
    async (
      assignment: Assignment,
      outcome: DispositionOutcome,
      endSession: boolean,
      // The compact sheet shows its own error and is the confirmation for
      // its outcome, so its failures must not leave an inspector Retry that
      // would resend them without it.
      { inlineError = true }: { inlineError?: boolean } = {},
    ): Promise<DispositionResult> => {
      setDispositionBusy((current) => ({ ...current, [assignment.id]: true }))
      setDispositionErrors((current) => withoutKey(current, assignment.id))
      setActionError(null)
      try {
        // The user decided on this assignment a while ago (Undo window,
        // open sheet, earlier failure). Ending the session is guarded by
        // worker and runtime versions that move with every Herdr status
        // change, so send the latest ones.
        const sent = endSession
          ? withLatestSessionVersions(
              assignment,
              await reloadAssignment(assignment.id).catch(() => undefined),
            )
          : assignment
        const result = await submitDisposition({
          assignment: sent,
          commands: dispositionCommands.current,
          endSession,
          outcome,
          reload: () => reloadAssignment(assignment.id),
        })
        if (result.kind === 'failed') {
          if (inlineError) {
            setDispositionErrors((current) => ({
              ...current,
              [assignment.id]: { endSession, message: result.message, outcome },
            }))
          }
          return result
        }
        setAssignments((current) =>
          current.map((candidate) =>
            candidate.id === result.assignment.id
              ? result.assignment
              : candidate,
          ),
        )
        if (result.kind === 'committed') {
          const endedSession = result.disposed
            ? result.disposed.worker !== null
            : result.assignment.worker.desired_state === 'ended'
          setActionNotice(
            dispositionNotice(outcome, endedSession, result.disposed),
          )
        } else {
          setActionNotice(result.message)
        }
        await Promise.all([
          loadWorkers(),
          loadInventory(selectedSession),
        ]).catch(() => undefined)
        return result
      } finally {
        setDispositionBusy((current) => withoutKey(current, assignment.id))
      }
    },
    [loadInventory, loadWorkers, reloadAssignment, selectedSession],
  )

  const quickCompletion = useQuickCompletionWindow({
    onDropped: () =>
      setActionNotice(
        'Completion was not sent because the page was hidden. The work is still active.',
      ),
    onSend: (pending) =>
      void runDisposition(pending.assignment, 'completed', pending.endSession),
  })
  const { start: startQuickCompletionWindow, undo: undoQuickCompletion } =
    quickCompletion
  // The inspector shows its own "Completing… Undo"; any other pending quick
  // completion keeps its Undo in the notice area.
  const inspectorAssignmentId =
    selectedAgentGroup.length > 1
      ? null
      : (selectedAssignment?.id ?? selectedCandidateActiveAssignment?.id ?? null)
  const offscreenQuickCompletions = quickCompletion.pendingCompletions.filter(
    ({ assignment }) => assignment.id !== inspectorAssignmentId,
  )

  const startQuickCompletion = useCallback(
    (assignment: Assignment) => {
      setDispositionErrors((current) => withoutKey(current, assignment.id))
      setActionNotice(null)
      startQuickCompletionWindow(assignment, completeAndEndSession)
    },
    [completeAndEndSession, startQuickCompletionWindow],
  )

  const retryDisposition = useCallback(
    (assignment: Assignment) => {
      const failed = dispositionErrors[assignment.id]
      if (!failed) return
      void runDisposition(assignment, failed.outcome, failed.endSession)
    },
    [dispositionErrors, runDisposition],
  )

  const proposeWorkerDisposition = useCallback(
    (
      assignment: Assignment,
      mode: WorkerDispositionMode,
      returnFocus: HTMLElement | null,
    ) => {
      setWorkerDispositionError(null)
      setActionNotice(null)
      undoQuickCompletion(assignment.id)
      setWorkerDispositionProposal({
        assignment,
        deleteCommandId: crypto.randomUUID(),
        mode,
        returnFocus,
      })
    },
    [undoQuickCompletion],
  )

  const confirmWorkerDisposition = useCallback(
    async (choice: WorkerDispositionChoice) => {
      if (!workerDispositionProposal) return
      const { assignment, deleteCommandId, mode } = workerDispositionProposal
      const outcome: DispositionOutcome =
        choice === 'complete' ? 'completed' : 'cancelled'
      setWorkerDispositionBusy(choice)
      setWorkerDispositionError(null)
      try {
        const result = await runDisposition(assignment, outcome, true, {
          inlineError: false,
        })
        if (result.kind === 'failed') {
          setWorkerDispositionError(result.message)
          return
        }
        if (result.kind === 'other_outcome' || mode === 'end') {
          setWorkerDispositionProposal(null)
          return
        }
        const workerId = assignment.worker.id
        const deleted = () => {
          setWorkerCandidates((current) =>
            current.filter(({ worker }) => worker.id !== workerId),
          )
          setSelection(null)
          setWorkerDispositionProposal(null)
        }
        let workerVersion = result.disposed?.worker?.version
        if (!workerVersion) {
          const workers = await loadWorkers()
          const candidate = workers.find(({ worker }) => worker.id === workerId)
          if (!candidate) {
            deleted()
            setActionNotice('Worker deleted from Yard.')
            return
          }
          workerVersion = candidate.worker.version
        }
        try {
          const removed = await deleteWorker(workerId, {
            command_id: deleteCommandId,
            actor: 'local-user',
            expected_worker_version: workerVersion,
          })
          deleted()
          setActionNotice(
            removed.cleanup_pending
              ? 'Worker deleted from Yard. Runtime cleanup continues in the background; the agent keeps running until its Herdr tab is closed.'
              : 'Worker deleted from Yard.',
          )
        } catch (caught) {
          const workers = await loadWorkers().catch(() => null)
          if (workers && !workers.some(({ worker }) => worker.id === workerId)) {
            deleted()
            setActionNotice(
              'Worker deleted from Yard. Runtime cleanup continues in the background if needed.',
            )
          } else {
            setWorkerDispositionError(
              caught instanceof Error ? caught.message : 'Delete worker failed',
            )
          }
        }
      } finally {
        setWorkerDispositionBusy(null)
      }
    },
    [loadWorkers, runDisposition, workerDispositionProposal],
  )

  const assignmentControls = useCallback(
    (assignment: Assignment): AssignmentDispositionControls => ({
      busy: Boolean(dispositionBusy[assignment.id]),
      completeAndEndSession,
      error: dispositionErrors[assignment.id]?.message ?? null,
      onComplete: () => startQuickCompletion(assignment),
      onCompleteAndEndSessionChange: setCompleteAndEndSession,
      onCompleteWithDetails: () =>
        setCompletionProposal({
          assignment,
          commandId: crypto.randomUUID(),
        }),
      onDelete: (trigger) =>
        proposeWorkerDisposition(assignment, 'delete', trigger),
      onEndSession: (trigger) =>
        proposeWorkerDisposition(assignment, 'end', trigger),
      onRetry: dispositionErrors[assignment.id]
        ? () => retryDisposition(assignment)
        : undefined,
      onUndo: () => undoQuickCompletion(assignment.id),
      pending: quickCompletion.isPending(assignment.id),
    }),
    [
      completeAndEndSession,
      dispositionBusy,
      dispositionErrors,
      proposeWorkerDisposition,
      quickCompletion,
      retryDisposition,
      startQuickCompletion,
      undoQuickCompletion,
    ],
  )

  // Fetches a preview for an open project confirmation and returns the new
  // command ID it is tied to; a result for an older command ID is ignored.
  const loadProjectDispositionPreview = useCallback(
    (
      projectId: string,
      setProposal: Dispatch<SetStateAction<ProjectDispositionProposal | null>>,
    ) => {
      const commandId = crypto.randomUUID()
      const settle = (
        update: Partial<{
          alreadyArchived: boolean
          preview: ProjectDispositionPreview
          previewError: string
        }>,
      ) =>
        setProposal((current) =>
          current?.commandId === commandId ? { ...current, ...update } : current,
        )
      fetchProjectDispositionPreview(projectId).then(
        (preview) => settle({ preview }),
        async (caught: unknown) => {
          const previewError =
            caught instanceof Error
              ? caught.message
              : 'Yard could not check what this affects'
          // The preview only covers active projects. One archived elsewhere
          // after this map loaded can still be deleted (not archived again).
          if (
            caught instanceof YardApiError &&
            caught.code === 'project_not_found'
          ) {
            const archived = await fetchArchivedProjects().catch(() => null)
            if (
              archived?.projects.some(
                (summary) => summary.project_id === projectId,
              )
            ) {
              settle({
                alreadyArchived: true,
                previewError: 'This project was already archived elsewhere.',
              })
              return
            }
          }
          settle({ previewError })
        },
      )
      return commandId
    },
    [],
  )

  // "Check again" after a preview could not be read.
  const recheckProjectDispositionPreview = useCallback(
    (
      proposal: ProjectDispositionProposal,
      setProposal: Dispatch<SetStateAction<ProjectDispositionProposal | null>>,
    ) => {
      const commandId = loadProjectDispositionPreview(
        proposal.project.id,
        setProposal,
      )
      setProposal((current) =>
        current?.commandId === proposal.commandId
          ? {
              ...current,
              alreadyArchived: false,
              commandId,
              preview: null,
              previewError: null,
            }
          : current,
      )
    },
    [loadProjectDispositionPreview],
  )

  // After a refusal that committed nothing: show the fresh preview the
  // server sent (or re-read it) under a new command ID, and ask again.
  const refreshProjectDispositionProposal = useCallback(
    (
      proposal: ProjectDispositionProposal,
      caught: YardApiError,
      setProposal: Dispatch<SetStateAction<ProjectDispositionProposal | null>>,
      setError: (message: string) => void,
    ) => {
      const fresh = isProjectDispositionPreview(caught.preview)
        ? caught.preview
        : null
      const commandId = fresh
        ? crypto.randomUUID()
        : loadProjectDispositionPreview(proposal.project.id, setProposal)
      setProposal((current) =>
        current?.commandId === proposal.commandId
          ? {
              ...current,
              alreadyArchived: false,
              commandId,
              preview: fresh,
              previewError: null,
            }
          : current,
      )
      setError(
        PROJECT_PREVIEW_REFRESH_CODES.has(caught.code)
          ? 'The active workers changed while this dialog was open. Review the updated list, then confirm again.'
          : 'This project changed while the dialog was open. Review the updated impact, then confirm again.',
      )
    },
    [loadProjectDispositionPreview],
  )

  const proposeProjectArchive = useCallback(
    (project: Project, returnFocus: HTMLButtonElement) => {
      setProjectArchiveError(null)
      setActionNotice(null)
      setProjectArchiveProposal({
        commandId: loadProjectDispositionPreview(
          project.id,
          setProjectArchiveProposal,
        ),
        preview: null,
        previewError: null,
        project,
        returnFocus,
      })
    },
    [loadProjectDispositionPreview],
  )

  // Undo an archive from its toast or the Archived shelf. Restore never
  // reopens cancelled assignments; the notice says so.
  const restoreArchive = useCallback(
    async (target: {
      archiveCommandId: string
      projectId: string
      projectName: string
    }) => {
      const key = target.archiveCommandId
      const commandId = (restoreCommandIds.current[key] ??=
        crypto.randomUUID())
      setProjectRestores((current) => ({
        ...current,
        [key]: { busy: true, error: null, retryable: false },
      }))
      setActionError(null)
      try {
        const result = await restoreProject(target.projectId, {
          command_id: commandId,
          actor: 'local-user',
          expected_archive_command_id: key,
        })
        delete restoreCommandIds.current[key]
        setProjectRestores((current) => {
          const next = { ...current }
          delete next[key]
          return next
        })
        setArchiveUndo((current) =>
          current?.archiveCommandId === key ? null : current,
        )
        setArchivedProjects(
          (current) =>
            current?.filter((summary) => summary.archive_command_id !== key) ??
            current,
        )
        setActionNotice(projectRestoreNotice(target.projectName, result))
        await Promise.all([
          loadProjects(),
          loadCoordination(),
          loadWorkers(),
          loadInventory(selectedSession),
          loadArchivedProjects(),
        ]).catch(() => undefined)
      } catch (caught) {
        const retryable = projectRestoreRetryable(caught)
        setProjectRestores((current) => ({
          ...current,
          [key]: {
            busy: false,
            error: projectRestoreErrorMessage(caught),
            retryable,
          },
        }))
        // Only Herdr being down is worth retrying unchanged. Any other
        // refusal is shown for a full Undo window and then expires.
        if (!retryable) {
          setArchiveUndo((current) =>
            current?.archiveCommandId === key
              ? { ...current, shownAt: Date.now() }
              : current,
          )
        }
        // Show the current archive state.
        void loadArchivedProjects()
        if (
          caught instanceof YardApiError &&
          caught.code === 'project_not_archived'
        ) {
          void loadProjects().catch(() => undefined)
        }
      }
    },
    [
      loadArchivedProjects,
      loadCoordination,
      loadInventory,
      loadProjects,
      loadWorkers,
      selectedSession,
    ],
  )

  const archiveUndoRestore = archiveUndo
    ? projectRestores[archiveUndo.archiveCommandId]
    : undefined
  // A retryable failure keeps the toast (with Retry) until dismissed.
  const archiveUndoHeld =
    archiveUndoRestore?.busy ||
    (Boolean(archiveUndoRestore?.error) && archiveUndoRestore?.retryable)
  useEffect(() => {
    if (!archiveUndo || archiveUndoHeld) {
      return
    }
    const timer = window.setTimeout(
      () => setArchiveUndo(null),
      Math.max(0, archiveUndo.shownAt + ARCHIVE_UNDO_WINDOW_MS - Date.now()),
    )
    return () => window.clearTimeout(timer)
  }, [archiveUndo, archiveUndoHeld])

  useEffect(() => {
    if (!resourceShelfOpen || railView !== 'archived') return
    const controller = new AbortController()
    void loadArchivedProjects(controller.signal)
    return () => controller.abort()
  }, [loadArchivedProjects, railView, resourceShelfOpen])

  const archiveSelectedProject = useCallback(async () => {
    if (!projectArchiveProposal?.preview) return
    const { commandId, preview, project } = projectArchiveProposal
    setProjectArchiveBusy(true)
    setProjectArchiveError(null)
    setActionError(null)

    const reconcileArchivedProject = async (result: {
      cleanup_pending: boolean
      background?: ProjectBackgroundStatus
      cancelled_assignment_ids?: string[]
      restorable?: boolean
    }) => {
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
      setActionNotice(projectDispositionNotice('archived', result))
      // Undo only when the server says this archive can be restored.
      setArchiveUndo(
        result.restorable
          ? {
              archiveCommandId: commandId,
              projectId: project.id,
              projectName: project.name,
              shownAt: Date.now(),
            }
          : null,
      )
      await Promise.all([
        loadProjects(),
        loadCoordination(),
        loadWorkers(),
        loadInventory(selectedSession),
        loadArchivedProjects(),
      ]).catch(() => undefined)
    }

    try {
      const result = await archiveProject(project.id, {
        command_id: commandId,
        actor: 'local-user',
        ...projectArchivePreconditions(preview),
      })
      await reconcileArchivedProject(result)
    } catch (caught) {
      const message =
        caught instanceof Error ? caught.message : 'Project archive failed'
      if (
        caught instanceof YardApiError &&
        (PROJECT_PREVIEW_REFRESH_CODES.has(caught.code) ||
          PROJECT_VERSION_REFRESH_CODES.has(caught.code))
      ) {
        refreshProjectDispositionProposal(
          projectArchiveProposal,
          caught,
          setProjectArchiveProposal,
          setProjectArchiveError,
        )
        return
      }
      const activeProjects = await fetchProjects().catch(() => null)
      if (
        activeProjects &&
        !activeProjects.projects.some(
          (candidate) => candidate.id === project.id,
        )
      ) {
        await reconcileArchivedProject({
          cleanup_pending: true,
          cancelled_assignment_ids: projectDispositionEndedAssignments(
            preview,
          ).map((assignment) => assignment.assignment_id),
        })
      } else {
        setProjectArchiveError(message)
      }
    } finally {
      setProjectArchiveBusy(false)
    }
  }, [
    clearProjectTransferContext,
    loadArchivedProjects,
    loadCoordination,
    loadInventory,
    loadProjects,
    loadWorkers,
    projectArchiveProposal,
    refreshProjectDispositionProposal,
    selectedSession,
  ])

  const proposeProjectDelete = useCallback(
    (project: Project, returnFocus: HTMLButtonElement) => {
      setProjectDeleteError(null)
      setActionNotice(null)
      setProjectDeleteProposal({
        commandId: loadProjectDispositionPreview(
          project.id,
          setProjectDeleteProposal,
        ),
        preview: null,
        previewError: null,
        project,
        returnFocus,
      })
    },
    [loadProjectDispositionPreview],
  )

  const deleteSelectedProject = useCallback(async () => {
    if (
      !projectDeleteProposal?.preview &&
      !projectDeleteProposal?.alreadyArchived
    ) {
      return
    }
    const { commandId, preview, project } = projectDeleteProposal
    setProjectDeleteBusy(true)
    setProjectDeleteError(null)
    setActionError(null)

    const reconcileDeletedProject = async (result: {
      cleanup_pending: boolean
      background?: ProjectBackgroundStatus
      cancelled_assignment_ids?: string[]
    }) => {
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
      setActionNotice(projectDispositionNotice('deleted', result))
      // Deletion is permanent, so its archive can no longer be undone.
      setArchiveUndo((current) =>
        current?.projectId === project.id ? null : current,
      )
      setArchivedProjects(
        (current) =>
          current?.filter((summary) => summary.project_id !== project.id) ??
          current,
      )
      await Promise.all([
        loadProjects(),
        loadCoordination(),
        loadWorkers(),
        loadInventory(selectedSession),
      ]).catch(() => undefined)
    }

    try {
      // One request archives the project if it is still active; a retry
      // reuses the command ID, so it replays even if another tab archived it.
      const result = await deleteProject(project.id, {
        command_id: commandId,
        actor: 'local-user',
        // Already archived: the server ignores archive preconditions.
        archive: preview ? projectArchivePreconditions(preview) : null,
      })
      await reconcileDeletedProject(result)
    } catch (caught) {
      const message =
        caught instanceof Error ? caught.message : 'Delete project failed'
      if (
        caught instanceof YardApiError &&
        (PROJECT_PREVIEW_REFRESH_CODES.has(caught.code) ||
          PROJECT_VERSION_REFRESH_CODES.has(caught.code))
      ) {
        refreshProjectDispositionProposal(
          projectDeleteProposal,
          caught,
          setProjectDeleteProposal,
          setProjectDeleteError,
        )
        return
      }
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
        await reconcileDeletedProject({
          cleanup_pending: true,
          cancelled_assignment_ids: projectDispositionEndedAssignments(
            preview,
          ).map((assignment) => assignment.assignment_id),
        })
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
    refreshProjectDispositionProposal,
    selectedSession,
  ])

  // Fetches a preview for the open sheet and returns the new command ID it is
  // tied to; a result for an older command ID is ignored.
  const loadWorkstreamDispositionPreview = useCallback((nodeId: string) => {
    const commandId = crypto.randomUUID()
    const settle = (
      update: Partial<{
        preview: CoordinationNodeDispositionPreview
        previewError: string
      }>,
    ) =>
      setWorkstreamDispositionProposal((current) =>
        current?.commandId === commandId ? { ...current, ...update } : current,
      )
    fetchCoordinationNodeDispositionPreview(nodeId).then(
      (preview) => settle({ preview }),
      (caught: unknown) =>
        settle({
          previewError:
            caught instanceof Error
              ? caught.message
              : 'Yard could not check what this affects',
        }),
    )
    return commandId
  }, [])

  const proposeWorkstreamDisposition = useCallback(
    (
      mode: 'archive' | 'delete',
      node: CoordinationNode,
      returnFocus: HTMLButtonElement,
    ) => {
      setWorkstreamDispositionError(null)
      setActionNotice(null)
      setWorkstreamDispositionProposal({
        commandId: loadWorkstreamDispositionPreview(node.id),
        mode,
        node,
        preview: null,
        previewError: null,
        returnFocus,
      })
    },
    [loadWorkstreamDispositionPreview],
  )

  const confirmWorkstreamDisposition = useCallback(async () => {
    const proposal = workstreamDispositionProposal
    if (!proposal?.preview) return
    const { commandId, mode, node, preview } = proposal
    setWorkstreamDispositionBusy(true)
    setWorkstreamDispositionError(null)
    setActionError(null)

    const reconcileDisposition = async (result: {
      worker_id: string | null
      cleanup_pending: boolean
      paused_automation_ids: string[]
    }) => {
      setCoordinationNodes((current) => {
        const next = current.filter((candidate) => candidate.id !== node.id)
        coordinationNodesRef.current = next
        return next
      })
      setCoordinationNodeRoutes((current) =>
        current.filter((route) => route.node_id !== node.id),
      )
      setCoordinationSnapshots((current) => {
        const next = { ...current }
        delete next[node.id]
        return next
      })
      if (mode === 'delete' && result.worker_id) {
        setWorkerCandidates((current) =>
          current.filter(({ worker }) => worker.id !== result.worker_id),
        )
      }
      // The workstream's automations leave the map with it (the server stops
      // listing them); they stay paused and durable for a later restore.
      const scopedToNode = (automation: Automation) =>
        automation.scope.kind === 'workstream_coordination_node' &&
        automation.scope.node_id === node.id
      const hiddenAutomationIds = new Set(
        automationsRef.current.filter(scopedToNode).map(({ id }) => id),
      )
      setAutomations((current) =>
        current.filter((automation) => !scopedToNode(automation)),
      )
      setSelection((current) =>
        (current?.kind === 'coordination-node' && current.id === node.id) ||
        (current?.kind === 'automation' && hiddenAutomationIds.has(current.id))
          ? null
          : current,
      )
      setWorkstreamDispositionProposal(null)
      setActionNotice(
        workstreamDispositionNotice(
          mode === 'archive' ? 'archived' : 'deleted',
          result,
        ),
      )
      await Promise.all([
        loadCoordination(),
        loadWorkers(),
        loadAutomations(),
        loadInventory(selectedSession),
      ]).catch(() => undefined)
    }

    // Only the node version is sent: it pins the dedicated worker, whose own
    // version moves with every runtime status change. Member projects are
    // not part of the request; archive and delete leave them untouched.
    const preconditions = { expected_node_version: preview.node_version }
    try {
      const result =
        mode === 'archive'
          ? await archiveCoordinationNode(node.id, {
              command_id: commandId,
              actor: 'local-user',
              ...preconditions,
            })
          : await deleteCoordinationNode(node.id, {
              command_id: commandId,
              actor: 'local-user',
              archive: preconditions,
            })
      await reconcileDisposition(result)
    } catch (caught) {
      const message =
        caught instanceof Error
          ? caught.message
          : `Workstream ${mode === 'archive' ? 'archive' : 'delete'} failed`
      if (
        caught instanceof YardApiError &&
        caught.code === 'coordination_node_version_conflict'
      ) {
        // Nothing committed under this command ID. Resending the same stale
        // version would fail the same way, so re-check the impact under a
        // new command ID and let the user confirm again.
        const refreshedCommandId = loadWorkstreamDispositionPreview(node.id)
        setWorkstreamDispositionProposal((current) =>
          current?.commandId === commandId
            ? {
                ...current,
                commandId: refreshedCommandId,
                preview: null,
                previewError: null,
              }
            : current,
        )
        setWorkstreamDispositionError(
          'This workstream changed while the dialog was open. Review the updated impact, then confirm again.',
        )
        return
      }
      // The response may have been lost after the command committed: an
      // archived node reads as a conflict, a deleted one as not found.
      const committed = await fetchCoordinationNode(node.id).then(
        () => false,
        (error: unknown) =>
          error instanceof YardApiError &&
          (error.code === 'coordination_node_not_found' ||
            (mode === 'archive' && error.code === 'coordination_node_archived')),
      )
      if (committed) {
        await reconcileDisposition({
          worker_id: preview.worker?.worker_id ?? null,
          cleanup_pending: preview.worker?.runtime_present ?? false,
          paused_automation_ids: preview.automations
            .filter((automation) => automation.state === 'active')
            .map((automation) => automation.automation_id),
        })
      } else {
        setWorkstreamDispositionError(message)
      }
    } finally {
      setWorkstreamDispositionBusy(false)
    }
  }, [
    loadAutomations,
    loadCoordination,
    loadInventory,
    loadWorkers,
    loadWorkstreamDispositionPreview,
    selectedSession,
    workstreamDispositionProposal,
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

  const createSummaryWorker = useCallback(
    async (project: Project, profile: WorkerProfile) => {
      setSummaryWorkerBusy(true)
      setSummaryWorkerError(null)
      try {
        const summary = await requestSummaryWorker(project.id, {
          command_id: crypto.randomUUID(),
          actor: 'local-user',
          parent_worker_id: project.orchestrator.id,
          expected_parent_worker_version: project.orchestrator.version,
          expected_project_version: project.version,
          profile_id: profile.id,
          expected_profile_version: profile.version,
          artifact_id: crypto.randomUUID(),
          objective:
            'Summarize the current project status, decisions, evidence, risks, and next steps for the parent orchestrator.',
        })
        setSummaryWorkers((current) => [
          summary,
          ...current.filter(
            (candidate) => candidate.command_id !== summary.command_id,
          ),
        ])
        const assignment = summary.assignment
        if (assignment) {
          setAssignments((current) => [
            ...current.filter(
              (candidate) => candidate.id !== assignment.id,
            ),
            assignment,
          ])
        }
        await Promise.all([
          loadInventory(selectedSession),
          loadWorkers(),
        ])
      } catch (caught) {
        setSummaryWorkerError(
          caught instanceof Error
            ? caught.message
            : 'Summary worker creation failed',
        )
      } finally {
        setSummaryWorkerBusy(false)
      }
    },
    [loadInventory, loadWorkers, selectedSession],
  )

  const handoffSummaryWorker = useCallback(
    async (project: Project, summary: SummaryWorker) => {
      if (!summary.assignment) return
      setSummaryWorkerBusy(true)
      setSummaryWorkerError(null)
      try {
        const received = await receiveSummaryWorker(
          project.id,
          summary.assignment.id,
          {
            command_id: crypto.randomUUID(),
            actor: 'local-user',
            expected_parent_worker_version: project.orchestrator.version,
          },
        )
        setSummaryWorkers((current) => [
          received.summary,
          ...current.filter(
            (candidate) =>
              candidate.command_id !== received.summary.command_id,
          ),
        ])
        const assignment = received.summary.assignment
        if (assignment) {
          setAssignments((current) => [
            ...current.filter(
              (candidate) => candidate.id !== assignment.id,
            ),
            assignment,
          ])
          setSelection({
            kind: 'assignment',
            id: assignment.id,
          })
        }
        await loadInventory(selectedSession)
      } catch (caught) {
        setSummaryWorkerError(
          caught instanceof Error ? caught.message : 'Summary handoff failed',
        )
      } finally {
        setSummaryWorkerBusy(false)
      }
    },
    [loadInventory, selectedSession],
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
      <div
        className="app-shell"
        data-shelf-open={resourceShelfOpen && !herdrInventoryOpen}
      >
        <GlobalCommandBar
          attentionCount={attentionCount}
          automaticCoordinationEnabledCount={automaticCoordination.length}
          automaticCoordinationLabel={`Automatic coordination enabled: ${automaticCoordination.join(
            ', ',
          )}`}
          busy={runtimeLoading || projectLoading}
          health={runtimeHealth}
          herdrInventoryOpen={herdrInventoryOpen}
          herdrInventoryTriggerRef={herdrInventoryTrigger}
          onHome={() => {
            setHerdrInventoryOpen(false)
            setAgentWorkspaceMode('map')
          }}
          onCreateProject={() => {
            setHerdrInventoryOpen(false)
            setSelection(null)
            setWorkspaceProjectOpen(true)
          }}
          onOpenHerdrInventory={() =>
            setHerdrInventoryOpen((open) => !open)
          }
          onOpenAttention={(trigger) => {
            resourceShelfTrigger.current = trigger
            setHerdrInventoryOpen(false)
            setRailView('workers')
            setFilter('attention')
            setResourceShelfOpen(true)
          }}
          onOpenAutomaticCoordination={() => {
            setSettingsInitialFocus('automatic')
            setSettingsOpen(true)
          }}
          onOpenProjectPulse={(trigger) => {
            projectPulseTrigger.current = trigger
            setProjectPulseOpen(true)
          }}
          onOpenSettings={() => {
            setSettingsInitialFocus('appearance')
            setSettingsOpen(true)
          }}
          onRefresh={() => void refresh()}
          onToggleResources={(trigger) => {
            resourceShelfTrigger.current = trigger
            setHerdrInventoryOpen(false)
            setResourceShelfOpen((open) => !open)
          }}
          onWorkspaceModeChange={(mode, trigger) => {
            agentWorkspaceReturnFocus.current = trigger
            setHerdrInventoryOpen(false)
            if (mode === 'terminal') {
              setTerminalPresentation(DEFAULT_TERMINAL_PRESENTATION)
            }
            setAgentWorkspaceMode(mode)
          }}
          onSessionChange={chooseSession}
          projectPulseTriggerRef={projectPulseTrigger}
          resourceShelfOpen={resourceShelfOpen}
          selectedSession={selectedSession}
          sessionRoleCounts={sessionRoleCounts}
          sessions={sessions}
          settingsLabel={`Settings, ${themeDefinition(theme).label} theme, ${
            mapVisualMode === 'depth' ? '2.5D' : '2D'
          } map, ${
            automaticCoordination.length === 0
              ? 'automatic coordination off'
              : `${automaticCoordination.length} automatic coordination behaviors enabled`
          }`}
          settingsTriggerRef={settingsTrigger}
          workspaceMode={agentWorkspaceMode}
        />

        {resourceShelfOpen && !herdrInventoryOpen ? (
          <section
            aria-label={`${railView} shelf`}
            className="resource-shelf"
            id="resource-shelf"
            onKeyDown={(event) => {
              if (event.key !== 'Escape') return
              event.preventDefault()
              setResourceShelfOpen(false)
              window.requestAnimationFrame(() =>
                resourceShelfTrigger.current?.focus(),
              )
            }}
            ref={resourceShelf}
          >
          <ResourceShelfTabs
            counts={{
              profiles: profiles.length,
              workers: workerCandidates.length,
              workspaces: availableWorkspaces.length,
              archived: archivedProjects?.length ?? null,
            }}
            onChange={setRailView}
            value={railView}
          />
          <button
            aria-label="Collapse resource shelf"
            className="icon-button resource-shelf__collapse"
            onClick={() => {
              setResourceShelfOpen(false)
              window.requestAnimationFrame(() =>
                resourceShelfTrigger.current?.focus(),
              )
            }}
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
                </div>
              ) : null}
            </div>
          </div>
        ) : railView === 'workers' ? (
          <div className="rail-section rail-section--resources" role="tabpanel">
            <div className="worker-shelf-controls">
              <div className="section-heading">
                <div>
                  <p className="eyebrow">Allocation</p>
                  <h2>Workers</h2>
                </div>
                <span>{visibleCandidates.length}</span>
                <button
                  aria-label="Review completed runtimes, read-only; nothing will be closed"
                  className="icon-button section-heading__action"
                  onClick={() => void openCompletedRuntimeCleanupPreview()}
                  ref={completedRuntimeCleanupPreviewTrigger}
                  title="Review completed runtimes, read-only; nothing will be closed"
                  type="button"
                >
                  <CircleHelp aria-hidden="true" size={16} />
                </button>
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
                    {option.value === 'stale' ? (
                      <small>{staleCandidates.length}</small>
                    ) : null}
                  </button>
                ))}
              </div>
              {filter === 'stale' ? (
                <button
                  className="secondary-button worker-shelf-hide-stale"
                  disabled={hideableStaleCandidates.length === 0}
                  onClick={(event) =>
                    proposeHideStaleWorkers(event.currentTarget)
                  }
                  type="button"
                >
                  <Trash2 aria-hidden="true" size={14} />
                  Hide stale ({hideableStaleCandidates.length})
                </button>
              ) : null}
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
                      (selection?.kind === 'worker' &&
                        selection.id === candidate.worker.id) ||
                      (candidate.availability === 'yard_orchestrator' &&
                        selection?.kind === 'yard-orchestrator') ||
                      (candidate.availability === 'orchestrator' &&
                        selection?.kind === 'orchestrator' &&
                        selection.projectId === candidate.project_id)
                    }
                    draggable={draggable}
                    key={candidate.worker.id}
                    onClick={() => {
                      if (candidate.availability === 'yard_orchestrator') {
                        setSelection({ kind: 'yard-orchestrator' })
                      } else if (
                        candidate.availability === 'orchestrator' &&
                        candidate.project_id
                      ) {
                        setSelection({
                          kind: 'orchestrator',
                          projectId: candidate.project_id,
                        })
                      } else {
                        setSelection({
                          kind: 'worker',
                          id: candidate.worker.id,
                        })
                      }
                    }}
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
                      <strong title={workerListLabels[candidate.worker.id]}>
                        {workerListLabels[candidate.worker.id]}
                      </strong>
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
        ) : railView === 'archived' ? (
          <ArchivedProjectsPanel
            archived={archivedProjects}
            error={archivedProjectsError}
            onRestore={(summary) =>
              void restoreArchive({
                archiveCommandId: summary.archive_command_id,
                projectId: summary.project_id,
                projectName: summary.name,
              })
            }
            restores={projectRestores}
          />
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
          workerDisplayNameByRuntimeId={workerDisplayNameByRuntimeId}
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
          workerLabels={workerLabels}
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

        {/* A background error never hides a fresh action notice: an action
            that committed during a Herdr outage still reports its result. */}
        <div className="canvas-banners">
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
        ) : null}
        {offscreenQuickCompletions.length > 0 ? (
          // A quick Complete stays undoable after its worker is no longer
          // in the inspector, for example while completing several in a row.
          <div
            aria-label="Pending completions"
            className="quick-complete-toasts"
            role="region"
          >
            {offscreenQuickCompletions.map(({ assignment }) => (
              <div
                className="notice-banner quick-complete-toast"
                key={assignment.id}
                role="status"
              >
                <LoaderCircle
                  aria-hidden="true"
                  className="status-spin"
                  size={18}
                />
                <span>Completing “{assignment.objective}”…</span>
                <button
                  className="secondary-button"
                  onClick={() => undoQuickCompletion(assignment.id)}
                  type="button"
                >
                  Undo
                </button>
              </div>
            ))}
          </div>
        ) : null}
        {archiveUndo ? (
          <div
            aria-label="Undo archive"
            className="archive-undo-toast"
            data-error={Boolean(archiveUndoRestore?.error)}
            role="status"
          >
            <RotateCcw aria-hidden="true" size={18} />
            <span>
              {archiveUndoRestore?.error ??
                `Archived “${archiveUndo.projectName}”.`}
            </span>
            {archiveUndoRestore?.error && !archiveUndoRestore.retryable ? null : (
              <button
                className="secondary-button"
                disabled={archiveUndoRestore?.busy}
                onClick={() => void restoreArchive(archiveUndo)}
                type="button"
              >
                {archiveUndoRestore?.busy
                  ? 'Restoring…'
                  : archiveUndoRestore?.error
                    ? 'Retry'
                    : 'Undo'}
              </button>
            )}
            <button
              aria-label="Dismiss undo"
              className="icon-button"
              onClick={() => setArchiveUndo(null)}
              title="Dismiss"
              type="button"
            >
              <X aria-hidden="true" size={16} />
            </button>
          </div>
        ) : null}
        {actionNotice ? (
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
        </div>
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
            defaultLabel={
              selectedYardOrchestrator.worker
                ? workerDefaultLabels[selectedYardOrchestrator.worker.id] ??
                  workerDefaultLabel({
                    projectOrchestratorName: 'Yard',
                    workerId: selectedYardOrchestrator.worker.id,
                  })
                : 'Yard orchestrator'
            }
            label={
              selectedYardOrchestrator.worker
                ? workerLabels[selectedYardOrchestrator.worker.id] ??
                  workerDisplayLabel({
                   displayName: selectedYardOrchestrator.worker.display_name,
                    projectOrchestratorName: 'Yard',
                    workerId: selectedYardOrchestrator.worker.id,
                  })
                : 'Yard orchestrator'
            }
            onCoordinationChange={recordYardRoute}
            onProvision={provisionCentralOrchestrator}
            onRecover={() => void recoverCentralOrchestrator()}
            onRefresh={() => void refresh()}
            onRenameWorker={renameWorkerLabel}
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
            onArchive={(node, trigger) =>
              proposeWorkstreamDisposition('archive', node, trigger)
            }
            onDelete={(node, trigger) =>
              proposeWorkstreamDisposition('delete', node, trigger)
            }
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
            busy={summaryWorkerBusy}
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
            label={
              workerLabels[selectedProjectOrchestrator.orchestrator.id] ??
              workerDisplayLabel({
                displayName: selectedProjectOrchestrator.orchestrator.display_name,
                projectOrchestratorName: selectedProjectOrchestrator.name,
                workerId: selectedProjectOrchestrator.orchestrator.id,
              })
            }
            onChange={(trigger) =>
              selectedProjectOrchestratorContextProject &&
              proposeProjectOrchestratorTransfer(
                selectedProjectOrchestratorContextProject,
                trigger,
              )
            }
            onCreateSummary={(profile) =>
              void createSummaryWorker(
                selectedProjectOrchestratorContextProject ??
                  selectedProjectOrchestrator,
                profile,
              )
            }
            onRefresh={() => void refresh()}
            onReceiveSummary={(summary) =>
              void handoffSummaryWorker(
                selectedProjectOrchestratorContextProject ??
                  selectedProjectOrchestrator,
                summary,
              )
            }
            onReplace={(trigger) =>
              proposeProjectOrchestratorReplacement(
                selectedProjectOrchestratorContextProject ??
                  selectedProjectOrchestrator,
                trigger,
              )
            }
            project={
              selectedProjectOrchestratorContextProject ??
              selectedProjectOrchestrator
            }
            profiles={profiles}
            statusReport={
              projectStatusReports[selectedProjectOrchestrator.id]
            }
            summaries={summaryWorkers.filter(
              (summary) =>
                summary.project_id === selectedProjectOrchestrator.id &&
                summary.parent_worker_id ===
                  selectedProjectOrchestrator.orchestrator.id,
            )}
            summaryError={summaryWorkerError}
          />
        ) : selectedProject ? (
          <ProjectInspector
            accent={resolvedProjectAccents[selectedProject.id]}
            inventory={inventory}
            orchestratorLabel={
              workerLabels[selectedProject.orchestrator.id] ??
              workerDisplayLabel({
                displayName: selectedProject.orchestrator.display_name,
                projectOrchestratorName: selectedProject.name,
                workerId: selectedProject.orchestrator.id,
              })
            }
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
            defaultLabel={
              workerDefaultLabels[selectedAssignment.worker.id] ??
              workerDefaultLabel({
                assignmentRole: selectedAssignment.role,
                profileName: selectedAssignment.profile_name,
                projectName: projects.find(
                  (project) => project.id === selectedAssignment.project_id,
                )?.name,
                workerId: selectedAssignment.worker.id,
              })
            }
            onRenameWorker={renameWorkerLabel}
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
            controls={assignmentControls(selectedAssignment)}
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
            activeControls={
              selectedCandidateActiveAssignment
                ? assignmentControls(selectedCandidateActiveAssignment)
                : null
            }
            candidate={selectedWorkerCandidate}
            completedAssignment={selectedCandidateCompletion}
            hideOnly={selectedWorkerAttentionState === 'stale'}
            inventory={inventory}
            defaultLabel={workerDefaultLabels[selectedWorkerCandidate.worker.id]}
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
              proposeWorkerDelete(
                selectedWorkerCandidate,
                selectedWorkerAttentionState === 'stale',
              )
            }
            onRefresh={() => void refresh()}
            onRename={renameWorkerLabel}
            projects={projects}
            snapshotCurrent={inventoryCurrent}
          />
        ) : selectedObservedWorker ? (
          <ObservedWorkerInspector
            label={workerLabel(
              selectedObservedWorker,
              inventory,
              visibleWorkers.map((worker) => worker.runtime_id),
              workerDisplayNameByRuntimeId[selectedObservedWorker.runtime_id],
            )}
            worker={selectedObservedWorker}
          />
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
            workerNames={workerDisplayNameByRuntimeId}
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
          automaticCoordinationBusy={tokenSpendSettingsBusy}
          automaticCoordinationError={tokenSpendSettingsError}
          automaticCoordinationSettings={tokenSpendSettings}
          completeAndEndSession={completeAndEndSession}
          initialFocus={settingsInitialFocus}
          mapVisualMode={mapVisualMode}
          onClose={() => setSettingsOpen(false)}
          onCompleteAndEndSessionChange={setCompleteAndEndSession}
          onMapVisualModeChange={setMapVisualMode}
          onOpenOrchestratorWorkflow={() => {
            setSettingsOpen(false)
            setOrchestratorWorkflowError(null)
            setOrchestratorWorkflowOpen(true)
          }}
          onSaveAutomaticCoordination={(selection) =>
            void saveTokenSpendSettings(selection)
          }
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
      <AgentWorkspaceShell
        activeTarget={activeAgentWorkspaceTarget}
        assignments={assignments}
        coordinationRoutes={coordinationNodeRoutes}
        mode={agentWorkspaceMode}
        onCoordinationChange={recordYardRoute}
        onCoordinationNodeChange={recordCoordinationNodeRoute}
        onModeChange={setAgentWorkspaceMode}
        onOpenProjectRepositories={(projectId) => {
          setAgentWorkspaceMode('map')
          setSelection({ kind: 'project', id: projectId })
          window.setTimeout(
            () =>
              document
                .getElementById(`project-repositories-${projectId}`)
                ?.focus(),
            0,
          )
        }}
        onPresentationChange={setTerminalPresentation}
        onRefresh={() => void refresh()}
        onRecoverTarget={(target, trigger) => {
          if (target.target.kind === 'orchestrator') {
            proposeProjectOrchestratorReplacement(
              target.target.project,
              trigger,
            )
          } else if (target.target.kind === 'yard-orchestrator') {
            void recoverCentralOrchestrator()
          }
        }}
        onTargetChange={(target) => {
          setAgentWorkspaceTarget({
            ...target,
            returnFocus: agentWorkspaceReturnFocus.current,
          })
          setTerminalPresentation(DEFAULT_TERMINAL_PRESENTATION)
        }}
        presentation={terminalPresentation}
        inventory={inventory}
        inventoryCurrent={inventoryCurrent}
        lensEntries={lensEntries}
        projects={projects}
        selectedSession={selectedSession}
        sessions={sessions}
        targets={agentWorkspaceTargets}
        returnFocus={agentWorkspaceReturnFocus.current}
        yardRoutes={yardOrchestratorRoutes}
      />
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
      {projectOrchestratorReplacement &&
      projectOrchestratorReplacementProject ? (
        <ProjectOrchestratorReplacementDialog
          busy={projectOrchestratorReplacementBusy}
          error={projectOrchestratorReplacementError}
          onClose={() => {
            if (projectOrchestratorReplacementBusy) return
            setProjectOrchestratorReplacementError(null)
            setProjectOrchestratorReplacement(null)
          }}
          onConfirm={replaceSelectedProjectOrchestrator}
          profiles={profiles}
          project={projectOrchestratorReplacementProject}
          returnFocus={projectOrchestratorReplacement.returnFocus}
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
          busy={projectArchiveBusy}
          error={projectArchiveError}
          onClose={() => {
            if (projectArchiveBusy) return
            setProjectArchiveError(null)
            setProjectArchiveProposal(null)
          }}
          alreadyArchived={projectArchiveProposal.alreadyArchived ?? false}
          onCheckAgain={() =>
            recheckProjectDispositionPreview(projectArchiveProposal, setProjectArchiveProposal)
          }
          onConfirm={archiveSelectedProject}
          preview={projectArchiveProposal.preview}
          previewError={projectArchiveProposal.previewError}
          project={projectArchiveProposal.project}
          returnFocus={projectArchiveProposal.returnFocus}
        />
      ) : null}
      {projectDeleteProposal ? (
        <DeleteProjectDialog
          busy={projectDeleteBusy}
          error={projectDeleteError}
          onClose={() => {
            if (projectDeleteBusy) return
            setProjectDeleteError(null)
            setProjectDeleteProposal(null)
          }}
          alreadyArchived={projectDeleteProposal.alreadyArchived ?? false}
          onCheckAgain={() =>
            recheckProjectDispositionPreview(projectDeleteProposal, setProjectDeleteProposal)
          }
          onConfirm={deleteSelectedProject}
          preview={projectDeleteProposal.preview}
          previewError={projectDeleteProposal.previewError}
          project={projectDeleteProposal.project}
          returnFocus={projectDeleteProposal.returnFocus}
        />
      ) : null}
      {completedRuntimeCleanupPreviewOpen ? (
        <CompletedRuntimeCleanupPreviewDialog
          error={completedRuntimeCleanupPreviewError}
          loading={completedRuntimeCleanupPreviewLoading}
          onClose={() => setCompletedRuntimeCleanupPreviewOpen(false)}
          preview={completedRuntimeCleanupPreview}
          returnFocus={completedRuntimeCleanupPreviewTrigger.current}
        />
      ) : null}
      {workstreamDispositionProposal ? (
        <WorkstreamDispositionDialog
          busy={workstreamDispositionBusy}
          error={workstreamDispositionError}
          mode={workstreamDispositionProposal.mode}
          node={workstreamDispositionProposal.node}
          onClose={() => {
            if (workstreamDispositionBusy) return
            setWorkstreamDispositionError(null)
            setWorkstreamDispositionProposal(null)
          }}
          onConfirm={confirmWorkstreamDisposition}
          preview={workstreamDispositionProposal.preview}
          previewError={workstreamDispositionProposal.previewError}
          returnFocus={workstreamDispositionProposal.returnFocus}
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
      {workerDispositionProposal ? (
        <WorkerDispositionSheet
          assignment={workerDispositionProposal.assignment}
          busy={workerDispositionBusy}
          error={workerDispositionError}
          mode={workerDispositionProposal.mode}
          onClose={() => {
            if (workerDispositionBusy) return
            setWorkerDispositionError(null)
            setWorkerDispositionProposal(null)
          }}
          onConfirm={confirmWorkerDisposition}
          returnFocus={workerDispositionProposal.returnFocus}
        />
      ) : null}
      {workerDeleteProposal ? (
        <DeleteWorkerDialog
          busy={workerDeleteBusy}
          candidate={workerDeleteProposal.candidate}
          error={workerDeleteError}
          hideOnly={workerDeleteProposal.hideOnly}
          label={
            workerLabels[workerDeleteProposal.candidate.worker.id] ??
            candidateLabel(workerDeleteProposal.candidate)
          }
          onClose={() => {
            if (workerDeleteBusy) return
            setWorkerDeleteError(null)
            setWorkerDeleteProposal(null)
          }}
          onConfirm={deleteSelectedWorker}
        />
      ) : null}
      {staleHideProposal ? (
        <HideStaleWorkersDialog
          busy={staleHideBusy}
          error={staleHideError}
          items={staleHideProposal.items.map(({ candidate }) => ({
            id: candidate.worker.id,
            label:
              workerLabels[candidate.worker.id] ??
              candidateLabel(candidate),
          }))}
          onClose={() => {
            if (staleHideBusy) return
            setStaleHideError(null)
            setStaleHideProposal(null)
          }}
          onConfirm={hideStaleWorkers}
          returnFocus={staleHideProposal.returnFocus}
        />
      ) : null}
      {herdrInventoryOpen ? (
        <HerdrInventoryWorkspace onClose={() => setHerdrInventoryOpen(false)} />
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

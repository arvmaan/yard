import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from 'react'
import {
  Controls,
  Handle,
  MiniMap,
  NodeResizer,
  Panel,
  Position,
  ReactFlow,
  useNodesState,
  MarkerType,
  type Connection,
  type Edge,
  type Node,
  type NodeChange,
  type NodeProps,
  type NodeTypes,
  type ReactFlowInstance,
  type ResizeParams,
} from '@xyflow/react'
import {
  Activity,
  Bot,
  CircleAlert,
  CircleCheck,
  CircleHelp,
  Clock3,
  Crown,
  Database,
  GitBranch,
  GripVertical,
  LoaderCircle,
  Network,
  Orbit,
  Pause,
  Radio,
} from 'lucide-react'
import type {
  Assignment,
  Automation,
  AutomationScope,
  CanvasPlacement,
  CoordinationNode,
  CoordinationNodeKind,
  CoordinationNodeRoute,
  ObservedChildAgent,
  ObservedStatus,
  ObservedWorker,
  Project,
  ProjectRelationship,
  ProviderSessionRef,
  RuntimeInventory,
  StatusReport,
  Worker,
  WorkerRuntimeBinding,
  WorkflowStatus,
  WorkspaceObservation,
  YardOrchestrator,
  YardOrchestratorRoute,
} from './types'
import type { ProjectStatusReports } from './projectUpdates'
import {
  ALLOCATION_DRAG_TYPE,
  getAllocationDragData,
  setAllocationDragData,
  type AllocationDragPayload,
} from './allocationDrag'
import { projectHeight } from './projectLayout'
import type { YardTheme } from './theme'

export type CanvasSelection =
  | { kind: 'yard-orchestrator' }
  | { kind: 'automation'; id: string }
  | { kind: 'coordination-node'; id: string }
  | { kind: 'orchestrator'; projectId: string }
  | { kind: 'project'; id: string }
  | { kind: 'workspace'; id: string }
  | { kind: 'observed-worker'; id: string }
  | { kind: 'provider-child'; id: string }
  | { kind: 'worker'; id: string }
  | { kind: 'assignment'; id: string }
  | { kind: 'profile'; id: string }
  | {
      kind: 'agent-group'
      targets: CanvasAgentTarget[]
    }
  | null

export type CanvasAgentTarget =
  | { kind: 'assignment'; id: string }
  | { kind: 'orchestrator'; projectId: string }

interface RuntimeCanvasProps {
  allocationPayloadByRuntimeId: Record<string, AllocationDragPayload>
  assignments: Assignment[]
  automations: Automation[]
  coordinationNodes: CoordinationNode[]
  coordinationNodeRoutes: CoordinationNodeRoute[]
  inventory: RuntimeInventory | null
  projectAccents: Record<string, string>
  projectRelationships: ProjectRelationship[]
  projectStatusReports: ProjectStatusReports
  projects: Project[]
  runtimeLoading: boolean
  selectedSession: string
  theme: YardTheme
  visibleWorkers: ObservedWorker[]
  yardOrchestrator: YardOrchestrator | null
  yardOrchestratorRoutes: YardOrchestratorRoute[]
  onAllocationDrop: (
    payload: AllocationDragPayload,
    projectId: string,
  ) => void
  onAutomationPlacementChange: (
    automation: Automation,
    placement: CanvasPlacement,
  ) => void
  onCoordinationNodeConnect: (nodeId: string, projectId: string) => void
  onCoordinationNodePlacementChange: (
    node: CoordinationNode,
    placement: CanvasPlacement,
  ) => void
  onCreateCoordinationNode: (
    placement: CanvasPlacement,
    kind: CoordinationNodeKind,
  ) => void
  onCreateAutomation: (
    placement: CanvasPlacement,
    initialScope?: AutomationScope,
  ) => void
  onProjectPlacementChange: (
    project: Project,
    placement: CanvasPlacement,
  ) => void
  onProjectConnect: (
    sourceProjectId: string,
    targetProjectId: string,
  ) => void
  onOpenProjectPulse: (trigger: HTMLButtonElement) => void
  onSelectionChange: (selection: CanvasSelection) => void
}

interface ProjectNodeData extends Record<string, unknown> {
  project: Project
  accent: string
  buildingCount: number
  completedBuildingCount: number
  workspace: WorkspaceObservation | null
  runtimePending: boolean
  visibleWorkerCount: number
  childAgentCount: number
  minimumHeight: number
  minimumWidth: number
  isAllocationTarget: boolean
  onPlacementChange: (
    project: Project,
    placement: CanvasPlacement,
  ) => void
}

interface ObservedWorkerNodeData extends Record<string, unknown> {
  allocationPayload?: AllocationDragPayload
  worker: ObservedWorker
}

interface WorkspaceNodeData extends Record<string, unknown> {
  workspace: WorkspaceObservation
  workerCount: number
  childAgentCount: number
}

interface OrchestratorNodeData extends Record<string, unknown> {
  observed: ObservedWorker | null
  project: Project
  statusReport: StatusReport | null
  worker: Worker
}

interface YardOrchestratorNodeData extends Record<string, unknown> {
  observed: ObservedWorker | null
  orchestrator: YardOrchestrator
}

interface CoordinationNodeData extends Record<string, unknown> {
  node: CoordinationNode
}

interface AutomationNodeData extends Record<string, unknown> {
  automation: Automation
}

interface AssignedWorkerNodeData extends Record<string, unknown> {
  assignment: Assignment
  observed: ObservedWorker | null
}

interface ChildAgentNodeData extends Record<string, unknown> {
  agent: ObservedChildAgent
  parentNodeId: string
  accent: string
}

interface CanvasPoint {
  x: number
  y: number
}

interface CanvasContextMenuEvent {
  clientX: number
  clientY: number
  preventDefault: () => void
  target: EventTarget | null
}

type ProjectNode = Node<ProjectNodeData, 'project'>
type WorkspaceNode = Node<WorkspaceNodeData, 'workspace'>
type ObservedWorkerNode = Node<ObservedWorkerNodeData, 'worker'>
type OrchestratorNode = Node<OrchestratorNodeData, 'orchestrator'>
type YardOrchestratorNode = Node<
  YardOrchestratorNodeData,
  'yard-orchestrator'
>
type CoordinationMapNode = Node<CoordinationNodeData, 'coordination-node'>
type AutomationMapNode = Node<AutomationNodeData, 'automation'>
type AssignedWorkerNode = Node<AssignedWorkerNodeData, 'assigned-worker'>
type ChildAgentNode = Node<ChildAgentNodeData, 'child-agent'>
type RuntimeNode =
  | ProjectNode
  | WorkspaceNode
  | ObservedWorkerNode
  | OrchestratorNode
  | YardOrchestratorNode
  | CoordinationMapNode
  | AutomationMapNode
  | AssignedWorkerNode
  | ChildAgentNode

const STATUS_ICONS = {
  blocked: CircleAlert,
  done: CircleCheck,
  idle: Pause,
  unknown: CircleHelp,
  working: LoaderCircle,
} satisfies Record<ObservedStatus, typeof CircleAlert>

const WORKFLOW_ICONS = {
  idle: Pause,
  needs_attention: CircleAlert,
  working: Radio,
} satisfies Record<WorkflowStatus, typeof CircleAlert>

const WORKFLOW_LABELS: Record<WorkflowStatus, string> = {
  idle: 'Flow idle',
  needs_attention: 'Needs attention',
  working: 'Flow working',
}

const WORKER_NODE_WIDTH = 101
const WORKER_NODE_HEIGHT = 115
const CHILD_NODE_WIDTH = 83
const CHILD_NODE_HEIGHT = 92
const AUTOMATION_NODE_WIDTH = 168
const AUTOMATION_NODE_HEIGHT = 58
const WORKER_GRID_COLUMNS = 3
const WORKER_GRID_START_X = 24
const WORKER_GRID_START_Y = 88
const WORKER_GRID_COLUMN_GAP = 16
const WORKER_GRID_ROW_GAP = 8
const TREE_COLUMN_GAP = 12
const TREE_SIBLING_GAP = 12
const TREE_GROUP_GAP = 28
const TREE_BOTTOM_GAP = 28
const WORKSPACE_POSITION_KEY_PREFIX = 'yard:workspace-positions:'
const AGENT_POSITION_KEY = 'yard:agent-positions:v1'

type CommunicationPathState = 'active' | 'failed' | 'idle'

// Delivery is visualized as active until the target emits the structured
// report correlated to that exact command. Transport success alone is not a
// response, while failed and ambiguous delivery both require attention.
function communicationPathState(
  route: YardOrchestratorRoute | CoordinationNodeRoute | undefined,
  report?: StatusReport | null,
): CommunicationPathState {
  if (!route) return 'idle'
  if (route.status === 'failed' || route.status === 'ambiguous') {
    return 'failed'
  }
  if (
    route.status === 'submitted' &&
    report?.command_id === route.command_id
  ) {
    return 'idle'
  }
  return 'active'
}

type MotionProperties = CSSProperties & {
  '--drift-delay': string
  '--drift-duration': string
  '--drift-x': string
  '--drift-y': string
}

type TerritoryProperties = CSSProperties & {
  '--project-accent': string
}

type BuildingProperties = CSSProperties & {
  '--building-height': string
  '--building-width': string
}

function stableHash(value: string) {
  let hash = 2166136261
  for (const character of value) {
    hash ^= character.charCodeAt(0)
    hash = Math.imul(hash, 16777619)
  }
  return hash >>> 0
}

function motionProperties(identity: string): MotionProperties {
  const hash = stableHash(identity)
  return {
    '--drift-delay': `${-((hash % 29) / 10)}s`,
    '--drift-duration': `${4.8 + ((hash >>> 4) % 24) / 10}s`,
    '--drift-x': `${2 + ((hash >>> 8) % 4)}px`,
    '--drift-y': `${2 + ((hash >>> 12) % 3)}px`,
  }
}

function cityBuildingStyles(
  projectId: string,
  buildingCount: number,
  completedBuildingCount: number,
): BuildingProperties[] {
  return Array.from({ length: buildingCount }, (_, index) => {
    const hash = stableHash(`${projectId}:building:${index}`)
    const isCompleted =
      index >= Math.max(1, buildingCount - completedBuildingCount)
    const height = Math.min(
      94,
      24 +
        (hash % 44) +
        Math.round(buildingCount * 1.4) +
        (isCompleted ? 12 : 0),
    )
    return {
      '--building-height': `${height}%`,
      '--building-width': `${7 + ((hash >>> 8) % 8)}%`,
    }
  })
}

function workerPosition(index: number) {
  return {
    x:
      WORKER_GRID_START_X +
      (index % WORKER_GRID_COLUMNS) *
        (WORKER_NODE_WIDTH + WORKER_GRID_COLUMN_GAP),
    y:
      WORKER_GRID_START_Y +
      Math.floor(index / WORKER_GRID_COLUMNS) *
        (WORKER_NODE_HEIGHT + WORKER_GRID_ROW_GAP),
  }
}

function workspacePositionKey(session: string) {
  return `${WORKSPACE_POSITION_KEY_PREFIX}${session}`
}

function readWorkspacePositions(session: string): Record<string, CanvasPoint> {
  if (!session) return {}
  try {
    const value = window.localStorage.getItem(workspacePositionKey(session))
    if (!value) return {}
    const parsed = JSON.parse(value) as Record<string, CanvasPoint>
    return Object.fromEntries(
      Object.entries(parsed).filter(
        ([, point]) =>
          Number.isFinite(point?.x) && Number.isFinite(point?.y),
      ),
    )
  } catch {
    return {}
  }
}

function writeWorkspacePositions(
  session: string,
  positions: Record<string, CanvasPoint>,
) {
  if (!session) return
  window.localStorage.setItem(
    workspacePositionKey(session),
    JSON.stringify(positions),
  )
}

function readAgentPositions(): Record<string, CanvasPoint> {
  try {
    const value = window.localStorage.getItem(AGENT_POSITION_KEY)
    if (!value) return {}
    const parsed = JSON.parse(value) as Record<string, CanvasPoint>
    return Object.fromEntries(
      Object.entries(parsed).filter(
        ([, point]) =>
          Number.isFinite(point?.x) && Number.isFinite(point?.y),
      ),
    )
  } catch {
    return {}
  }
}

function writeAgentPositions(positions: Record<string, CanvasPoint>) {
  window.localStorage.setItem(AGENT_POSITION_KEY, JSON.stringify(positions))
}

function selectionFromNodes(nodes: RuntimeNode[]): CanvasSelection {
  if (nodes.length === 0) return null

  const agentTargets = nodes.flatMap((node): CanvasAgentTarget[] => {
    if (node.type === 'assigned-worker') {
      const assignment = (node.data as AssignedWorkerNodeData).assignment
      return assignment.lifecycle === 'active'
        ? [{ kind: 'assignment', id: assignment.id }]
        : []
    }
    if (node.type === 'orchestrator') {
      return [
        {
          kind: 'orchestrator',
          projectId: (node.data as OrchestratorNodeData).project.id,
        },
      ]
    }
    return []
  })
  const uniqueTargets = [
    ...new Map(
      agentTargets.map((target) => [
        target.kind === 'assignment'
          ? `assignment:${target.id}`
          : `orchestrator:${target.projectId}`,
        target,
      ]),
    ).values(),
  ]
  if (uniqueTargets.length > 1) {
    return { kind: 'agent-group', targets: uniqueTargets }
  }

  const node = nodes.at(-1)
  if (!node) return null
  if (node.type === 'worker') {
    return {
      kind: 'observed-worker',
      id: (node.data as ObservedWorkerNodeData).worker.runtime_id,
    }
  }
  if (node.type === 'yard-orchestrator') {
    return { kind: 'yard-orchestrator' }
  }
  if (node.type === 'automation') {
    return {
      kind: 'automation',
      id: (node.data as AutomationNodeData).automation.id,
    }
  }
  if (node.type === 'coordination-node') {
    return {
      kind: 'coordination-node',
      id: (node.data as CoordinationNodeData).node.id,
    }
  }
  if (node.type === 'child-agent') {
    return {
      kind: 'provider-child',
      id: (node.data as ChildAgentNodeData).agent.runtime_id,
    }
  }
  if (node.type === 'assigned-worker') {
    return {
      kind: 'assignment',
      id: (node.data as AssignedWorkerNodeData).assignment.id,
    }
  }
  if (node.type === 'orchestrator') {
    return {
      kind: 'orchestrator',
      projectId: (node.data as OrchestratorNodeData).project.id,
    }
  }
  if (node.type === 'project') {
    return {
      kind: 'project',
      id: (node.data as ProjectNodeData).project.id,
    }
  }
  return {
    kind: 'workspace',
    id: (node.data as WorkspaceNodeData).workspace.runtime_id,
  }
}

function placementFromResize(params: ResizeParams): CanvasPlacement {
  return {
    x: params.x,
    y: params.y,
    width: params.width,
    height: params.height,
  }
}

function numericDimension(value: string | number | undefined, fallback: number) {
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (typeof value === 'string') {
    const parsed = Number.parseFloat(value)
    if (Number.isFinite(parsed)) return parsed
  }
  return fallback
}

function ProjectRegion({ data, selected }: NodeProps<ProjectNode>) {
  const {
    project,
    accent,
    buildingCount,
    completedBuildingCount,
    workspace,
    runtimePending,
    visibleWorkerCount,
    childAgentCount,
    minimumHeight,
    minimumWidth,
    isAllocationTarget,
    onPlacementChange,
  } = data

  return (
    <div
      className={`workspace-region project-region ${selected ? 'is-selected' : ''} ${isAllocationTarget ? 'is-allocation-target' : ''}`}
      data-runtime={
        workspace ? 'online' : runtimePending ? 'loading' : 'offline'
      }
      data-status={workspace?.status ?? 'unknown'}
      data-building-count={buildingCount}
      data-completed-building-count={completedBuildingCount}
      style={{ '--project-accent': accent } as TerritoryProperties}
    >
      <NodeResizer
        color={accent}
        isVisible={selected}
        maxHeight={2400}
        maxWidth={2400}
        minHeight={minimumHeight}
        minWidth={minimumWidth}
        onResizeEnd={(_, params) =>
          onPlacementChange(project, placementFromResize(params))
        }
      />
      <Handle
        aria-label={`${project.name} relationship target`}
        className="project-relationship-handle"
        id="relationship-target"
        position={Position.Left}
        type="target"
      />
      <Handle
        aria-label={`${project.name} relationship source`}
        className="project-relationship-handle"
        id="relationship-source"
        position={Position.Right}
        type="source"
      />
      <Handle
        aria-label={`${project.name} coordination target`}
        className="coordination-handle"
        id="coordination-target"
        position={Position.Top}
        type="target"
      />
      <div aria-hidden="true" className="city-silhouette">
        {cityBuildingStyles(
          project.id,
          buildingCount,
          completedBuildingCount,
        ).map((style, index) => (
          <span key={index} style={style} />
        ))}
      </div>
      <div className="workspace-region__heading">
        <div className="workspace-region__title">
          <span className="workspace-region__index">Y</span>
          <strong>{project.name}</strong>
        </div>
        <span className="workspace-region__count">
          {workspace
            ? `${visibleWorkerCount} worker${visibleWorkerCount === 1 ? '' : 's'}${childAgentCount > 0 ? ` + ${childAgentCount} child${childAgentCount === 1 ? '' : 'ren'}` : ''}`
            : runtimePending
              ? 'loading'
              : 'offline'}
        </span>
      </div>
      <div className="workspace-region__meta">
        <span>{project.runtime.session}</span>
        <span>
          {workspace
            ? `${workspace.tab_count} tabs`
            : runtimePending
              ? 'checking runtime'
              : 'not observed'}
        </span>
      </div>
    </div>
  )
}

function WorkspaceRegion({ data, selected }: NodeProps<WorkspaceNode>) {
  const { workspace, workerCount, childAgentCount } = data
  return (
    <div
      className={`workspace-region runtime-workspace-region ${selected ? 'is-selected' : ''}`}
      data-status={workspace.status}
    >
      <div className="workspace-region__heading">
        <div className="workspace-region__title">
          <span className="workspace-region__index">H</span>
          <strong>{workspace.label}</strong>
        </div>
        <span className="workspace-region__count">
          {workerCount} agent{workerCount === 1 ? '' : 's'}
          {childAgentCount > 0
            ? ` + ${childAgentCount} child${childAgentCount === 1 ? '' : 'ren'}`
            : ''}
        </span>
      </div>
      <div className="workspace-region__meta">
        <span>Herdr workspace</span>
        <span>{workspace.tab_count} tabs</span>
      </div>
    </div>
  )
}

function WorkerMarker({ data, selected }: NodeProps<ObservedWorkerNode>) {
  const { allocationPayload, worker } = data
  const StatusIcon = STATUS_ICONS[worker.status]
  const label =
    worker.name ?? worker.display_provider ?? worker.provider ?? 'Worker'

  return (
    <div className="worker-node-shell">
      <button
        aria-label={`Move ${label}`}
        className="worker-marker__placement-handle"
        onClick={(event) => event.stopPropagation()}
        title="Move agent"
        type="button"
      >
        <GripVertical aria-hidden="true" size={15} />
      </button>
      <div
        className={`worker-marker ${selected ? 'is-selected' : ''}`}
        data-draggable={Boolean(allocationPayload)}
        data-role="worker"
        data-status={worker.status}
        draggable={Boolean(allocationPayload)}
        onDragStart={(event) => {
          if (allocationPayload) {
            setAllocationDragData(event.dataTransfer, allocationPayload)
          }
        }}
        style={motionProperties(worker.runtime_id)}
      >
        <Handle
          className="provider-child-handle"
          id="child-source"
          position={Position.Right}
          type="source"
        />
        <span className="worker-marker__motion">
          <span className="worker-marker__body">
            <span className="worker-marker__glyph">
              <Bot aria-hidden="true" size={25} strokeWidth={1.8} />
            </span>
            <StatusIcon
              aria-label={`Observed status: ${worker.status}`}
              className={`worker-marker__status ${worker.status === 'working' ? 'status-spin' : ''}`}
              size={16}
              strokeWidth={2}
            />
          </span>
        </span>
        <span className="worker-marker__label">
          <strong>{label}</strong>
          <small className="worker-marker__runtime">{worker.status}</small>
        </span>
      </div>
    </div>
  )
}

function childAgentLabel(agent: ObservedChildAgent) {
  return (
    agent.name ??
    agent.description ??
    `${agent.provider} ${agent.provider_agent_id.slice(0, 8)}`
  )
}

function ChildAgentMarker({
  data,
  selected,
}: NodeProps<ChildAgentNode>) {
  const { agent, parentNodeId } = data
  const StatusIcon = STATUS_ICONS[agent.status]
  const label = childAgentLabel(agent)

  return (
    <button
      aria-label={`${label}, ${agent.provider} child agent, ${agent.status}`}
      className={`child-agent-marker nodrag nopan ${selected ? 'is-selected' : ''}`}
      data-depth={agent.depth}
      data-parent-node-id={parentNodeId}
      data-provider={agent.provider}
      data-status={agent.status}
      type="button"
    >
      <Handle
        className="provider-child-handle"
        id="child-target"
        position={Position.Left}
        type="target"
      />
      <Handle
        className="provider-child-handle"
        id="child-source"
        position={Position.Right}
        type="source"
      />
      <span className="child-agent-marker__glyph">
        <GitBranch aria-hidden="true" size={18} strokeWidth={1.8} />
        <StatusIcon
          aria-label={`Observed status: ${agent.status}`}
          className={agent.status === 'working' ? 'status-spin' : ''}
          size={13}
          strokeWidth={2}
        />
      </span>
      <span className="child-agent-marker__label">
        <strong>{label}</strong>
        <small>{agent.role ?? `${agent.provider} subagent`}</small>
      </span>
    </button>
  )
}

function resolvedRuntimeState(
  runtime: WorkerRuntimeBinding | null,
  observed: ObservedWorker | null,
) {
  return {
    processState:
      runtime?.process_state ??
      (observed ? ('running' as const) : ('unknown' as const)),
    status: observed?.status ?? runtime?.status ?? ('unknown' as const),
  }
}

function workerLabel(worker: ObservedWorker) {
  return worker.name ?? worker.display_provider ?? worker.provider ?? 'Worker'
}

function YardOrchestratorMarker({
  data,
  selected,
}: NodeProps<YardOrchestratorNode>) {
  const { observed, orchestrator } = data
  const worker = orchestrator.worker
  const runtimeState = resolvedRuntimeState(worker?.runtime ?? null, observed)
  const StatusIcon = STATUS_ICONS[runtimeState.status]
  const configured = worker !== null

  return (
    <div className="yard-hub-node">
      <button
        aria-label="Move Superintendent"
        className="yard-hub-node__placement-handle"
        onClick={(event) => event.stopPropagation()}
        title="Move Superintendent"
        type="button"
      >
        <GripVertical aria-hidden="true" size={15} />
      </button>
      <div
        className={`yard-orchestrator-marker ${selected ? 'is-selected' : ''}`}
        data-configured={configured}
        data-observation-state={
          observed
            ? 'observed'
            : (worker?.runtime?.observation_state ?? 'missing')
        }
        data-process-state={runtimeState.processState}
        data-status={runtimeState.status}
        data-worker-id={worker?.id ?? ''}
      >
        <Handle
          className="automation-handle"
          id="automation-target"
          position={Position.Left}
          type="target"
        />
        <Handle
          className="provider-child-handle"
          id="child-source"
          position={Position.Right}
          type="source"
        />
        <Handle
          className="coordination-handle"
          id="coordination-source"
          position={Position.Bottom}
          type="source"
        />
        <span className="yard-orchestrator-marker__glyph">
          <Network aria-hidden="true" size={35} strokeWidth={1.7} />
          <StatusIcon
            aria-label={`Observed status: ${runtimeState.status}`}
            className={runtimeState.status === 'working' ? 'status-spin' : ''}
            size={15}
            strokeWidth={2}
          />
        </span>
        <span className="yard-orchestrator-marker__label">
          <strong>Superintendent</strong>
          <small>{configured ? 'Portfolio control' : 'Configure'}</small>
        </span>
      </div>
    </div>
  )
}

function CoordinationNodeMarker({
  data,
  selected,
}: NodeProps<CoordinationMapNode>) {
  const { node } = data
  const Icon = node.kind === 'workstream' ? Network : Database
  const status = node.worker?.runtime?.status ?? 'unknown'
  const StatusIcon = STATUS_ICONS[status]

  return (
    <div
      className={`coordination-map-node ${selected ? 'is-selected' : ''}`}
      data-kind={node.kind}
      data-provisioned={Boolean(node.worker)}
      data-status={status}
    >
      <Handle
        className="automation-handle"
        id="automation-target"
        position={Position.Left}
        type="target"
      />
      <Handle
        className="coordination-handle"
        id="node-source"
        position={Position.Bottom}
        type="source"
      />
      <span className="coordination-map-node__halo" aria-hidden="true" />
      <span className="coordination-map-node__glyph">
        <Icon aria-hidden="true" size={27} strokeWidth={1.65} />
        {node.kind === 'workstream' ? (
          <StatusIcon
            aria-label={`Observed status: ${status}`}
            className={status === 'working' ? 'status-spin' : ''}
            size={12}
            strokeWidth={2}
          />
        ) : (
          <span aria-hidden="true">
            {node.attached_project_ids.length}
          </span>
        )}
      </span>
      <span className="coordination-map-node__label">
        <strong>{node.name}</strong>
        <small>
          {node.kind === 'workstream'
            ? node.worker
              ? 'Workstream'
              : 'Needs worker'
            : 'Knowledge store'}
        </small>
      </span>
    </div>
  )
}

function OrchestratorMarker({
  data,
  selected,
}: NodeProps<OrchestratorNode>) {
  const { observed, project, statusReport, worker } = data
  const runtimeState = resolvedRuntimeState(worker.runtime, observed)
  const StatusIcon = STATUS_ICONS[runtimeState.status]
  const WorkflowIcon = statusReport
    ? WORKFLOW_ICONS[statusReport.state]
    : null
  const label = observed ? workerLabel(observed) : 'Orchestrator'

  return (
    <div className="worker-node-shell">
      <button
        aria-label={`Move ${project.name} orchestrator`}
        className="worker-marker__placement-handle"
        onClick={(event) => event.stopPropagation()}
        title="Move agent"
        type="button"
      >
        <GripVertical aria-hidden="true" size={15} />
      </button>
      <div
        className={`worker-marker orchestrator-marker ${selected ? 'is-selected' : ''}`}
        data-observation-state={
          observed
            ? 'observed'
            : (worker.runtime?.observation_state ?? 'missing')
        }
        data-process-state={runtimeState.processState}
        data-project-id={project.id}
        data-role="orchestrator"
        data-status={runtimeState.status}
        data-workflow-state={statusReport?.state ?? 'unreported'}
        data-worker-id={worker.id}
        style={motionProperties(worker.id)}
      >
        <Handle
          className="automation-handle"
          id="automation-target"
          position={Position.Left}
          type="target"
        />
        <Handle
          className="allocation-handle"
          id="allocation-source"
          position={Position.Right}
          type="source"
        />
        <Handle
          className="provider-child-handle"
          id="child-source"
          position={Position.Right}
          type="source"
        />
        <span className="worker-marker__motion">
          <span className="worker-marker__body">
            <span className="worker-marker__glyph">
              <Crown aria-hidden="true" size={24} strokeWidth={1.8} />
            </span>
            <StatusIcon
              aria-label={`Observed status: ${runtimeState.status}`}
              className={`worker-marker__status ${runtimeState.status === 'working' ? 'status-spin' : ''}`}
              size={16}
              strokeWidth={2}
            />
          </span>
        </span>
        <span className="worker-marker__label">
          <strong>{label}</strong>
          <small
            className="worker-marker__workflow"
            data-workflow-state={statusReport?.state ?? 'unreported'}
          >
            {WorkflowIcon ? (
              <WorkflowIcon aria-hidden="true" size={9} />
            ) : null}
            {statusReport
              ? WORKFLOW_LABELS[statusReport.state]
              : 'Orchestrator'}
          </small>
        </span>
      </div>
    </div>
  )
}

function AssignedWorkerMarker({
  data,
  selected,
}: NodeProps<AssignedWorkerNode>) {
  const { assignment, observed } = data
  const runtimeState = resolvedRuntimeState(assignment.worker.runtime, observed)
  const StatusIcon = STATUS_ICONS[runtimeState.status]

  return (
    <div className="worker-node-shell">
      <button
        aria-label={`Move ${assignment.profile_name}`}
        className="worker-marker__placement-handle"
        onClick={(event) => event.stopPropagation()}
        title="Move agent"
        type="button"
      >
        <GripVertical aria-hidden="true" size={15} />
      </button>
      <div
        className={`worker-marker assigned-worker-marker nodrag nopan ${selected ? 'is-selected' : ''}`}
        data-draggable={assignment.lifecycle === 'active'}
        data-process-state={runtimeState.processState}
        data-role="assigned"
        data-status={runtimeState.status}
        data-worker-id={assignment.worker.id}
        draggable={assignment.lifecycle === 'active'}
        onDragStart={(event) => {
          event.stopPropagation()
          setAllocationDragData(event.dataTransfer, {
            kind: 'worker',
            id: assignment.worker.id,
            mode: 'handoff',
          })
        }}
        style={motionProperties(assignment.worker.id)}
      >
        <Handle
          className="allocation-handle"
          id="allocation-target"
          position={Position.Left}
          type="target"
        />
        <Handle
          className="provider-child-handle"
          id="child-source"
          position={Position.Right}
          type="source"
        />
        <span className="worker-marker__motion">
          <span className="worker-marker__body">
            <span className="worker-marker__glyph">
              <Bot aria-hidden="true" size={25} strokeWidth={1.8} />
            </span>
            <StatusIcon
              aria-label={`Observed status: ${runtimeState.status}`}
              className={`worker-marker__status ${runtimeState.status === 'working' ? 'status-spin' : ''}`}
              size={16}
              strokeWidth={2}
            />
          </span>
        </span>
        <span className="worker-marker__label">
          <strong>{assignment.profile_name}</strong>
          <small>
            {assignment.lifecycle === 'active' &&
            runtimeState.status === 'done'
              ? 'Review completion'
              : assignment.role}
          </small>
        </span>
      </div>
    </div>
  )
}

const AUTOMATION_RUN_LABELS = {
  ambiguous: 'Ambiguous',
  failed: 'Failed',
  pending: 'Pending',
  submitted: 'Submitted',
} as const

function AutomationMarker({
  data,
  selected,
}: NodeProps<AutomationMapNode>) {
  const { automation } = data
  const latestStatus = automation.latest_run?.status ?? 'never'

  return (
    <div
      className={`automation-map-node ${selected ? 'is-selected' : ''}`}
      data-run-status={latestStatus}
      data-state={automation.state}
    >
      <Handle
        className="automation-handle"
        id="automation-source"
        position={Position.Right}
        type="source"
      />
      <span className="automation-map-node__grip">
        <GripVertical aria-hidden="true" size={12} />
      </span>
      <span className="automation-map-node__glyph">
        <Clock3 aria-hidden="true" size={17} />
      </span>
      <span className="automation-map-node__label">
        <strong>{automation.name}</strong>
        <small>
          {String(automation.schedule.hour).padStart(2, '0')}:
          {String(automation.schedule.minute).padStart(2, '0')}
          {' / '}
          {automation.state === 'active' ? 'Active' : 'Paused'}
        </small>
      </span>
      <span
        className="automation-map-node__run"
        data-status={latestStatus}
      >
        {automation.latest_run
          ? AUTOMATION_RUN_LABELS[automation.latest_run.status]
          : 'No runs'}
      </span>
    </div>
  )
}

const NODE_TYPES: NodeTypes = {
  automation: AutomationMarker,
  'coordination-node': CoordinationNodeMarker,
  project: ProjectRegion,
  workspace: WorkspaceRegion,
  worker: WorkerMarker,
  orchestrator: OrchestratorMarker,
  'yard-orchestrator': YardOrchestratorMarker,
  'assigned-worker': AssignedWorkerMarker,
  'child-agent': ChildAgentMarker,
}

function observedWorkspace(
  project: Project,
  inventory: RuntimeInventory | null,
): WorkspaceObservation | null {
  if (
    !inventory ||
    inventory.adapter !== project.runtime.adapter ||
    inventory.session !== project.runtime.session
  ) {
    return null
  }
  return (
    inventory.workspaces.find(
      (workspace) => workspace.runtime_id === project.runtime.workspace_id,
    ) ?? null
  )
}

function observedWorker(
  project: Project,
  runtime: WorkerRuntimeBinding | null,
  inventory: RuntimeInventory | null,
): ObservedWorker | null {
  if (
    !runtime ||
    !inventory ||
    inventory.adapter !== project.runtime.adapter ||
    inventory.session !== project.runtime.session
  ) {
    return null
  }

  const matches = inventory.workers.filter(
    (worker) => worker.terminal_id === runtime.terminal_id,
  )
  return matches.length === 1 ? matches[0] : null
}

function observedRuntimeWorker(
  runtime: WorkerRuntimeBinding | null,
  inventory: RuntimeInventory | null,
): ObservedWorker | null {
  if (
    !runtime ||
    !inventory ||
    inventory.adapter !== runtime.adapter ||
    inventory.session !== runtime.session
  ) {
    return null
  }
  const matches = inventory.workers.filter(
    (worker) => worker.terminal_id === runtime.terminal_id,
  )
  return matches.length === 1 ? matches[0] : null
}

function projectAtPoint(nodes: RuntimeNode[], point: CanvasPoint) {
  const node = [...nodes].reverse().find((candidate) => {
    if (candidate.type !== 'project') return false
    const project = (candidate.data as ProjectNodeData).project
    const geometry = project.placement.geometry
    const width =
      candidate.measured?.width ??
      (typeof candidate.style?.width === 'number'
        ? candidate.style.width
        : Math.max(geometry.width, 350))
    const height =
      candidate.measured?.height ??
      (typeof candidate.style?.height === 'number'
        ? candidate.style.height
        : Math.max(geometry.height, projectHeight(1)))
    return (
      point.x >= candidate.position.x &&
      point.x <= candidate.position.x + width &&
      point.y >= candidate.position.y &&
      point.y <= candidate.position.y + height
    )
  })
  return node ? (node.data as ProjectNodeData).project : undefined
}

function automationTargetNodeId(scope: AutomationScope) {
  if (scope.kind === 'project_orchestrator') {
    return `orchestrator:${scope.project_id}`
  }
  if (scope.kind === 'workstream_coordination_node') {
    return `coordination-node:${scope.node_id}`
  }
  return 'yard-orchestrator'
}

function isVisibleAssignment(assignment: Assignment, project: Project) {
  return (
    (assignment.lifecycle === 'allocating' ||
      assignment.lifecycle === 'active' ||
      assignment.lifecycle === 'handing_off') &&
    assignment.worker.id !== project.orchestrator.id &&
    (!assignment.worker.runtime?.terminal_id ||
      assignment.worker.runtime.terminal_id !==
        project.orchestrator.runtime?.terminal_id)
  )
}

interface ChildAgentRoot {
  nodeId: string
  session: ProviderSessionRef | null
}

interface LinkedChildAgent {
  agent: ObservedChildAgent
  parentNodeId: string
}

function providerSessionMatches(
  left: ProviderSessionRef,
  right: ProviderSessionRef,
) {
  return (
    left.provider === right.provider &&
    left.kind === right.kind &&
    left.value === right.value
  )
}

function linkedChildAgents(
  inventory: RuntimeInventory | null,
  roots: ChildAgentRoot[],
): LinkedChildAgent[] {
  const availableChildren = inventory?.child_agents ?? []
  return roots.flatMap((root) => {
    const session = root.session
    if (!session) return []
    const children = availableChildren.filter(
      (agent) =>
        agent.status === 'working' &&
        providerSessionMatches(agent.parent_provider_session, session),
    )
    const childrenByProviderId = new Map(
      children.map((agent) => [
        `${agent.provider}:${agent.provider_agent_id}`,
        agent,
      ]),
    )
    return children.map((agent) => {
      const parent = agent.parent_agent_id
        ? childrenByProviderId.get(
            `${agent.provider}:${agent.parent_agent_id}`,
          )
        : undefined
      return {
        agent,
        parentNodeId:
          parent && parent.depth < agent.depth
            ? `child-agent:${parent.runtime_id}`
            : root.nodeId,
      }
    })
  })
}

interface AgentTreeLayout {
  positions: Map<string, CanvasPoint>
  minimumHeight: number
  minimumWidth: number
}

function childNodeId(agent: ObservedChildAgent) {
  return `child-agent:${agent.runtime_id}`
}

function layoutAgentTrees(
  roots: ChildAgentRoot[],
  linkedChildren: LinkedChildAgent[],
  savedPositions: Record<string, CanvasPoint>,
): AgentTreeLayout {
  const positions = new Map<string, CanvasPoint>()
  const childrenByParent = new Map<string, LinkedChildAgent[]>()
  for (const child of linkedChildren) {
    const siblings = childrenByParent.get(child.parentNodeId) ?? []
    siblings.push(child)
    childrenByParent.set(child.parentNodeId, siblings)
  }
  for (const siblings of childrenByParent.values()) {
    siblings.sort(
      (left, right) =>
        left.agent.depth - right.agent.depth ||
        childAgentLabel(left.agent).localeCompare(childAgentLabel(right.agent)),
    )
  }

  const subtreeHeight = (
    nodeId: string,
    ancestors = new Set<string>(),
  ): number => {
    if (ancestors.has(nodeId)) return CHILD_NODE_HEIGHT
    const children = childrenByParent.get(nodeId) ?? []
    if (children.length === 0) return CHILD_NODE_HEIGHT
    const nextAncestors = new Set(ancestors).add(nodeId)
    return Math.max(
      CHILD_NODE_HEIGHT,
      children.reduce(
        (total, child, index) =>
          total +
          subtreeHeight(childNodeId(child.agent), nextAncestors) +
          (index > 0 ? TREE_SIBLING_GAP : 0),
        0,
      ),
    )
  }

  let maximumRight = 350 - WORKER_GRID_START_X
  let maximumBottom = WORKER_GRID_START_Y
  const placedChildren = new Set<string>()

  const placeChildren = (
    parentNodeId: string,
    depth: number,
    blockTop: number,
    blockHeight: number,
    translation: CanvasPoint,
    ancestors = new Set<string>(),
  ) => {
    if (ancestors.has(parentNodeId)) return
    const children = childrenByParent.get(parentNodeId) ?? []
    if (children.length === 0) return
    const nextAncestors = new Set(ancestors).add(parentNodeId)
    const heights = children.map((child) =>
      subtreeHeight(childNodeId(child.agent), nextAncestors),
    )
    const childrenHeight =
      heights.reduce((total, height) => total + height, 0) +
      Math.max(0, children.length - 1) * TREE_SIBLING_GAP
    let childTop = blockTop + Math.max(0, (blockHeight - childrenHeight) / 2)

    children.forEach((child, index) => {
      const nodeId = childNodeId(child.agent)
      if (placedChildren.has(nodeId)) return
      placedChildren.add(nodeId)
      const height = heights[index]
      const position = {
        x:
          WORKER_GRID_START_X +
          WORKER_NODE_WIDTH +
          TREE_COLUMN_GAP +
          (depth - 1) * (CHILD_NODE_WIDTH + TREE_COLUMN_GAP) +
          translation.x,
        y:
          childTop +
          Math.max(0, (height - CHILD_NODE_HEIGHT) / 2) +
          translation.y,
      }
      positions.set(nodeId, position)
      maximumRight = Math.max(
        maximumRight,
        position.x + CHILD_NODE_WIDTH,
      )
      maximumBottom = Math.max(
        maximumBottom,
        position.y + CHILD_NODE_HEIGHT,
      )
      placeChildren(
        nodeId,
        depth + 1,
        childTop,
        height,
        translation,
        nextAncestors,
      )
      childTop += height + TREE_SIBLING_GAP
    })
  }

  const treeRoots = roots.filter((root) =>
    childrenByParent.has(root.nodeId),
  )
  const plainRoots = roots.filter(
    (root) => !childrenByParent.has(root.nodeId),
  )
  let cursorY = WORKER_GRID_START_Y

  for (const root of treeRoots) {
    const children = childrenByParent.get(root.nodeId) ?? []
    const childrenHeight =
      children.reduce(
        (total, child, index) =>
          total +
          subtreeHeight(childNodeId(child.agent)) +
          (index > 0 ? TREE_SIBLING_GAP : 0),
        0,
      )
    const treeHeight = Math.max(WORKER_NODE_HEIGHT, childrenHeight)
    const defaultPosition = {
      x: WORKER_GRID_START_X,
      y: cursorY + (treeHeight - WORKER_NODE_HEIGHT) / 2,
    }
    const position = savedPositions[root.nodeId] ?? defaultPosition
    const translation = {
      x: position.x - defaultPosition.x,
      y: position.y - defaultPosition.y,
    }
    positions.set(root.nodeId, position)
    maximumRight = Math.max(maximumRight, position.x + WORKER_NODE_WIDTH)
    maximumBottom = Math.max(
      maximumBottom,
      position.y + WORKER_NODE_HEIGHT,
    )
    placeChildren(
      root.nodeId,
      1,
      cursorY,
      treeHeight,
      translation,
    )
    cursorY += treeHeight + TREE_GROUP_GAP
  }

  const plainStartY =
    treeRoots.length > 0 ? cursorY : WORKER_GRID_START_Y
  plainRoots.forEach((root, index) => {
    const gridPosition = workerPosition(index)
    const defaultPosition = {
      x: gridPosition.x,
      y: plainStartY + gridPosition.y - WORKER_GRID_START_Y,
    }
    const position = savedPositions[root.nodeId] ?? defaultPosition
    positions.set(root.nodeId, position)
    maximumRight = Math.max(maximumRight, position.x + WORKER_NODE_WIDTH)
    maximumBottom = Math.max(
      maximumBottom,
      position.y + WORKER_NODE_HEIGHT,
    )
  })

  return {
    positions,
    minimumHeight: Math.max(
      projectHeight(roots.length),
      Math.ceil(maximumBottom + TREE_BOTTOM_GAP),
    ),
    minimumWidth: Math.max(
      350,
      Math.ceil(maximumRight + WORKER_GRID_START_X),
    ),
  }
}

function buildNodes(
  allocationPayloadByRuntimeId: Record<string, AllocationDragPayload>,
  assignments: Assignment[],
  automations: Automation[],
  coordinationNodes: CoordinationNode[],
  inventory: RuntimeInventory | null,
  projectAccents: Record<string, string>,
  projectStatusReports: ProjectStatusReports,
  projects: Project[],
  runtimeLoading: boolean,
  selectedSession: string,
  visibleWorkers: ObservedWorker[],
  yardOrchestrator: YardOrchestrator | null,
  allocationTargetId: string | null,
  workspacePositions: Record<string, CanvasPoint>,
  agentPositions: Record<string, CanvasPoint>,
  onPlacementChange: (
    project: Project,
    placement: CanvasPlacement,
  ) => void,
): RuntimeNode[] {
  const yardOrchestratorTerminalId =
    yardOrchestrator?.worker?.runtime?.terminal_id ?? null
  const coordinationTerminalIds = coordinationNodes
    .map((node) => node.worker?.runtime?.terminal_id)
    .filter((terminalId): terminalId is string => Boolean(terminalId))
  const projectNodes = projects.flatMap((project): RuntimeNode[] => {
    const workspace = observedWorkspace(project, inventory)
    const runtimePending =
      runtimeLoading &&
      (!selectedSession || project.runtime.session === selectedSession)
    const allProjectAssignments = assignments.filter(
      (assignment) => assignment.project_id === project.id,
    )
    const projectAssignments = allProjectAssignments.filter((assignment) =>
      isVisibleAssignment(assignment, project),
    )
    const assignedTerminalIds = new Set(
      allProjectAssignments
        .map((assignment) => assignment.worker.runtime?.terminal_id)
        .filter((terminalId): terminalId is string => Boolean(terminalId)),
    )
    if (project.orchestrator.runtime?.terminal_id) {
      assignedTerminalIds.add(project.orchestrator.runtime.terminal_id)
    }
    if (yardOrchestratorTerminalId) {
      assignedTerminalIds.add(yardOrchestratorTerminalId)
    }
    const workers = workspace
      ? visibleWorkers.filter(
          (worker) =>
            worker.workspace_id === workspace.runtime_id &&
            !assignedTerminalIds.has(worker.terminal_id),
        )
      : []
    const orchestratorNodeId = `orchestrator:${project.id}`
    const orchestratorObserved = observedWorker(
      project,
      project.orchestrator.runtime,
      inventory,
    )
    const childRoots: ChildAgentRoot[] = [
      {
        nodeId: orchestratorNodeId,
        session:
          orchestratorObserved?.provider_session ??
          project.orchestrator.runtime?.provider_session ??
          null,
      },
      ...workers.map((worker) => ({
        nodeId: `worker:${worker.runtime_id}`,
        session: worker.provider_session,
      })),
      ...projectAssignments.map((assignment) => {
        const observed = observedWorker(
          project,
          assignment.worker.runtime,
          inventory,
        )
        return {
          nodeId: `assigned-worker:${assignment.worker.id}`,
          session:
            observed?.provider_session ??
            assignment.worker.runtime?.provider_session ??
            null,
        }
      }),
    ]
    const linkedChildren = linkedChildAgents(inventory, childRoots)
    const rootWorkerCount =
      1 + workers.length + projectAssignments.length
    const visibleWorkerCount = rootWorkerCount
    const childAgentCount = linkedChildren.length
    const treeLayout = layoutAgentTrees(
      childRoots,
      linkedChildren,
      agentPositions,
    )
    const completedBuildingCount = allProjectAssignments.filter(
      (assignment) => assignment.completion_receipt !== null,
    ).length
    const artifactCount = allProjectAssignments.reduce(
      (total, assignment) =>
        total + (assignment.completion_receipt?.artifacts.length ?? 0),
      0,
    )
    const buildingCount = Math.min(
      18,
      3 +
        allProjectAssignments.length +
        Math.min(artifactCount, 6),
    )
    const minimumHeight = treeLayout.minimumHeight
    const minimumWidth = treeLayout.minimumWidth
    const geometry = project.placement.geometry
    const statusReport = projectStatusReports[project.id] ?? null
    const projectNodeId = `project:${project.id}`
    const accent = projectAccents[project.id] ?? '#19766b'
    const projectNode: ProjectNode = {
      id: projectNodeId,
      type: 'project',
      position: { x: geometry.x, y: geometry.y },
      data: {
        project,
        accent,
        buildingCount,
        completedBuildingCount,
        workspace,
        runtimePending,
        visibleWorkerCount,
        childAgentCount,
        minimumHeight,
        minimumWidth,
        isAllocationTarget: project.id === allocationTargetId,
        onPlacementChange,
      },
      dragHandle: '.workspace-region__heading',
      style: {
        width: Math.max(geometry.width, minimumWidth),
        height: Math.max(geometry.height, minimumHeight),
      },
      deletable: false,
      ariaLabel: `${project.name}, ${
        workspace ? 'observed' : runtimePending ? 'loading' : 'offline'
      }, ${visibleWorkerCount} visible workers, ${childAgentCount} live child agents, ${buildingCount} city structures, ${completedBuildingCount} completed`,
      focusable: true,
    }
    const orchestratorState = resolvedRuntimeState(
      project.orchestrator.runtime,
      orchestratorObserved,
    )
    const orchestratorNode: OrchestratorNode = {
      id: orchestratorNodeId,
      type: 'orchestrator',
      parentId: projectNodeId,
      extent: 'parent',
      expandParent: false,
      draggable: true,
      dragHandle: '.worker-marker__placement-handle',
      position:
        treeLayout.positions.get(orchestratorNodeId) ?? workerPosition(0),
      style: {
        width: WORKER_NODE_WIDTH,
        height: WORKER_NODE_HEIGHT,
      },
      deletable: false,
      data: {
        observed: orchestratorObserved,
        project,
        statusReport,
        worker: project.orchestrator,
      },
      ariaLabel: `${project.name} orchestrator, observed runtime ${orchestratorState.status}, process ${orchestratorState.processState}${
        statusReport
          ? `, reported workflow ${statusReport.state.replace('_', ' ')}`
          : ''
      }`,
      focusable: true,
    }
    const workerNodes: ObservedWorkerNode[] = workers.map(
      (worker, workerIndex) => {
        const nodeId = `worker:${worker.runtime_id}`
        return {
          id: nodeId,
          type: 'worker',
          parentId: projectNodeId,
          extent: 'parent',
          expandParent: false,
          draggable: true,
          dragHandle: '.worker-marker__placement-handle',
          position:
            treeLayout.positions.get(nodeId) ??
            workerPosition(workerIndex + 1),
          style: {
            width: WORKER_NODE_WIDTH,
            height: WORKER_NODE_HEIGHT,
          },
          deletable: false,
          data: {
            allocationPayload: allocationPayloadByRuntimeId[worker.runtime_id],
            worker,
          },
          ariaLabel: `${worker.name ?? worker.provider ?? 'Worker'}, ${worker.status}`,
          focusable: true,
        }
      },
    )
    const assignedNodes: AssignedWorkerNode[] = projectAssignments.map(
      (assignment, assignmentIndex) => {
        const observed = observedWorker(
          project,
          assignment.worker.runtime,
          inventory,
        )
        const workerIndex = workers.length + assignmentIndex + 1
        const runtimeState = resolvedRuntimeState(
          assignment.worker.runtime,
          observed,
        )
        const nodeId = `assigned-worker:${assignment.worker.id}`
        return {
          id: nodeId,
          type: 'assigned-worker',
          parentId: projectNodeId,
          extent: 'parent',
          expandParent: false,
          draggable: true,
          dragHandle: '.worker-marker__placement-handle',
          position:
            treeLayout.positions.get(nodeId) ?? workerPosition(workerIndex),
          style: {
            width: WORKER_NODE_WIDTH,
            height: WORKER_NODE_HEIGHT,
          },
          deletable: false,
          data: { assignment, observed },
          ariaLabel: `${assignment.profile_name}, ${assignment.role}, ${runtimeState.status}, process ${runtimeState.processState}, assignment ${assignment.lifecycle}, coordinated by the ${project.name} orchestrator`,
          focusable: true,
        }
      },
    )
    const childNodes: ChildAgentNode[] = linkedChildren.map(
      ({ agent, parentNodeId }, childIndex) => {
        const nodeId = `child-agent:${agent.runtime_id}`
        return {
          id: nodeId,
          type: 'child-agent',
          parentId: projectNodeId,
          extent: 'parent',
          expandParent: false,
          draggable: false,
          position:
            treeLayout.positions.get(nodeId) ??
            workerPosition(rootWorkerCount + childIndex),
          style: {
            width: CHILD_NODE_WIDTH,
            height: CHILD_NODE_HEIGHT,
          },
          deletable: false,
          data: { agent, parentNodeId, accent },
          ariaLabel: `${childAgentLabel(agent)}, ${agent.provider} child agent, ${agent.status}`,
          focusable: true,
        }
      },
    )

    return [
      projectNode,
      orchestratorNode,
      ...workerNodes,
      ...assignedNodes,
      ...childNodes,
    ]
  })

  const representedWorkspaceIds = new Set(
    projects
      .filter(
        (project) =>
          inventory &&
          project.runtime.adapter === inventory.adapter &&
          project.runtime.session === inventory.session,
      )
      .map((project) => project.runtime.workspace_id),
  )
  const representedTerminalIds = new Set(
    [
      yardOrchestratorTerminalId,
      ...coordinationTerminalIds,
      ...projects
        .filter(
          (project) =>
            inventory &&
            project.runtime.adapter === inventory.adapter &&
            project.runtime.session === inventory.session,
        )
        .map((project) => project.orchestrator.runtime?.terminal_id ?? null),
      ...assignments
        .filter(
          (assignment) =>
            inventory &&
            assignment.worker.runtime?.adapter === inventory.adapter &&
            assignment.worker.runtime.session === inventory.session &&
            (assignment.lifecycle === 'allocating' ||
              assignment.lifecycle === 'active' ||
              assignment.lifecycle === 'handing_off'),
        )
        .map((assignment) => assignment.worker.runtime?.terminal_id ?? null),
    ].filter((terminalId): terminalId is string => terminalId !== null),
  )
  const unboundWorkers =
    inventory?.session === selectedSession
      ? visibleWorkers.filter(
          (worker) =>
            !representedWorkspaceIds.has(worker.workspace_id) &&
            !representedTerminalIds.has(worker.terminal_id),
        )
      : []
  const workspaceStartY =
    projects.length === 0
      ? 96
      : Math.max(
          ...projects.map(
            (project) =>
              project.placement.geometry.y +
              Math.max(
                project.placement.geometry.height,
                projectHeight(1),
              ),
          ),
        ) + 48
  const workersByWorkspace = new Map<string, ObservedWorker[]>()
  for (const worker of unboundWorkers) {
    const workers = workersByWorkspace.get(worker.workspace_id) ?? []
    workers.push(worker)
    workersByWorkspace.set(worker.workspace_id, workers)
  }
  const workspaceNodes: RuntimeNode[] = []
  const columnBottoms = [workspaceStartY, workspaceStartY, workspaceStartY]
  const unboundWorkspaces =
    inventory?.session === selectedSession
      ? inventory.workspaces.filter(
          (workspace) =>
            !representedWorkspaceIds.has(workspace.runtime_id) &&
            (workersByWorkspace.get(workspace.runtime_id)?.length ?? 0) > 0,
        )
      : []
  unboundWorkspaces.forEach((workspace, workspaceIndex) => {
    const workers = workersByWorkspace.get(workspace.runtime_id) ?? []
    const childRoots = workers.map((worker) => ({
      nodeId: `worker:${worker.runtime_id}`,
      session: worker.provider_session,
    }))
    const linkedChildren = linkedChildAgents(
      inventory,
      childRoots,
    )
    const visibleAgentCount = workers.length + linkedChildren.length
    const treeLayout = layoutAgentTrees(
      childRoots,
      linkedChildren,
      agentPositions,
    )
    const column = workspaceIndex % 3
    const height = Math.max(
      projectHeight(visibleAgentCount),
      treeLayout.minimumHeight,
    )
    const width = treeLayout.minimumWidth
    const workspaceNodeId = `workspace:${workspace.runtime_id}`
    workspaceNodes.push({
      id: workspaceNodeId,
      type: 'workspace',
      position:
        workspacePositions[workspace.runtime_id] ?? {
          x: 32 + column * 430,
          y: columnBottoms[column],
        },
      draggable: true,
      dragHandle: '.workspace-region__heading',
      style: { width, height },
      deletable: false,
      data: {
        workspace,
        workerCount: workers.length,
        childAgentCount: linkedChildren.length,
      },
      ariaLabel: `${workspace.label}, ${visibleAgentCount} observed agents`,
      focusable: true,
    })
    workers.forEach((worker, workerIndex) => {
      const nodeId = `worker:${worker.runtime_id}`
      workspaceNodes.push({
        id: nodeId,
        type: 'worker',
        parentId: workspaceNodeId,
        extent: 'parent',
        draggable: true,
        dragHandle: '.worker-marker__placement-handle',
        position:
          treeLayout.positions.get(nodeId) ?? workerPosition(workerIndex),
        style: {
          width: WORKER_NODE_WIDTH,
          height: WORKER_NODE_HEIGHT,
        },
        deletable: false,
        data: {
          allocationPayload: allocationPayloadByRuntimeId[worker.runtime_id],
          worker,
        },
        ariaLabel: `${worker.name ?? worker.provider ?? 'Worker'}, ${worker.status}`,
        focusable: true,
      })
    })
    linkedChildren.forEach(({ agent, parentNodeId }, childIndex) => {
      const nodeId = `child-agent:${agent.runtime_id}`
      workspaceNodes.push({
        id: nodeId,
        type: 'child-agent',
        parentId: workspaceNodeId,
        extent: 'parent',
        draggable: false,
        position:
          treeLayout.positions.get(nodeId) ??
          workerPosition(workers.length + childIndex),
        style: {
          width: CHILD_NODE_WIDTH,
          height: CHILD_NODE_HEIGHT,
        },
        deletable: false,
        data: { agent, parentNodeId, accent: '#3178a8' },
        ariaLabel: `${childAgentLabel(agent)}, ${agent.provider} child agent, ${agent.status}`,
        focusable: true,
      })
    })
    columnBottoms[column] += height + 58
  })

  const yardNodes: RuntimeNode[] = []
  if (yardOrchestrator) {
    const nodeId = 'yard-orchestrator'
    const worker = yardOrchestrator.worker
    const observed = observedRuntimeWorker(worker?.runtime ?? null, inventory)
    const minimumProjectX =
      projects.length > 0
        ? Math.min(...projects.map((project) => project.placement.geometry.x))
        : 210
    const minimumProjectY =
      projects.length > 0
        ? Math.min(...projects.map((project) => project.placement.geometry.y))
        : 88
    const defaultPosition = {
      x: minimumProjectX - WORKER_NODE_WIDTH - 24,
      y: Math.min(220, Math.max(110, minimumProjectY - 110)),
    }
    const roots: ChildAgentRoot[] = [
      {
        nodeId,
        session:
          observed?.provider_session ??
          worker?.runtime?.provider_session ??
          null,
      },
    ]
    const linkedChildren = linkedChildAgents(inventory, roots)
    const treeLayout = layoutAgentTrees(roots, linkedChildren, {
      ...agentPositions,
      [nodeId]: agentPositions[nodeId] ?? defaultPosition,
    })
    const runtimeState = resolvedRuntimeState(
      worker?.runtime ?? null,
      observed,
    )
    yardNodes.push({
      id: nodeId,
      type: 'yard-orchestrator',
      position: treeLayout.positions.get(nodeId) ?? defaultPosition,
      draggable: true,
      dragHandle: '.yard-hub-node__placement-handle',
      style: {
        height: WORKER_NODE_HEIGHT,
        width: WORKER_NODE_WIDTH,
        zIndex: 8,
      },
      deletable: false,
      data: {
        observed,
        orchestrator: yardOrchestrator,
      },
      ariaLabel: worker
        ? `Superintendent, ${runtimeState.status}, process ${runtimeState.processState}`
        : 'Superintendent, not configured',
      focusable: true,
    } satisfies YardOrchestratorNode)
    linkedChildren.forEach(({ agent, parentNodeId }, index) => {
      const childId = childNodeId(agent)
      yardNodes.push({
        id: childId,
        type: 'child-agent',
        draggable: false,
        position:
          treeLayout.positions.get(childId) ??
          workerPosition(index + 1),
        style: {
          height: CHILD_NODE_HEIGHT,
          width: CHILD_NODE_WIDTH,
          zIndex: 7,
        },
        deletable: false,
        data: {
          accent: '#c64b3c',
          agent,
          parentNodeId,
        },
        ariaLabel: `${childAgentLabel(agent)}, ${agent.provider} child agent, ${agent.status}`,
        focusable: true,
      } satisfies ChildAgentNode)
    })
  }

  const coordinationMapNodes = coordinationNodes.map(
    (node): CoordinationMapNode => ({
      id: `coordination-node:${node.id}`,
      type: 'coordination-node',
      position: {
        x: node.placement.geometry.x,
        y: node.placement.geometry.y,
      },
      draggable: true,
      style: {
        height: node.placement.geometry.height,
        width: node.placement.geometry.width,
        zIndex: 6,
      },
      deletable: false,
      data: { node },
      ariaLabel: `${node.name}, ${
        node.kind === 'workstream' ? 'workstream orchestrator' : 'knowledge store'
      }, ${node.attached_project_ids.length} attached projects`,
      focusable: true,
    }),
  )

  const automationMapNodes = automations.map(
    (automation): AutomationMapNode => ({
      id: `automation:${automation.id}`,
      type: 'automation',
      position: {
        x: automation.placement.geometry.x,
        y: automation.placement.geometry.y,
      },
      draggable: true,
      dragHandle: '.automation-map-node__grip',
      style: {
        height: AUTOMATION_NODE_HEIGHT,
        width: AUTOMATION_NODE_WIDTH,
        zIndex: 9,
      },
      deletable: false,
      data: { automation },
      ariaLabel: `${automation.name}, ${automation.state}, latest run ${
        automation.latest_run?.status ?? 'none'
      }`,
      focusable: true,
    }),
  )

  return [
    ...yardNodes,
    ...automationMapNodes,
    ...coordinationMapNodes,
    ...projectNodes,
    ...workspaceNodes,
  ]
}

export function RuntimeCanvas({
  allocationPayloadByRuntimeId,
  assignments,
  automations,
  coordinationNodes,
  coordinationNodeRoutes,
  inventory,
  onAllocationDrop,
  onAutomationPlacementChange,
  onCoordinationNodeConnect,
  onCoordinationNodePlacementChange,
  onCreateCoordinationNode,
  onCreateAutomation,
  onOpenProjectPulse,
  onProjectConnect,
  projectAccents,
  projectRelationships,
  projectStatusReports,
  projects,
  runtimeLoading,
  selectedSession,
  theme,
  visibleWorkers,
  yardOrchestrator,
  yardOrchestratorRoutes,
  onProjectPlacementChange,
  onSelectionChange,
}: RuntimeCanvasProps) {
  const [contextMenu, setContextMenu] = useState<{
    automationPlacement?: CanvasPlacement
    initialScope?: AutomationScope
    left: number
    placement: CanvasPlacement
    targetLabel?: string
    top: number
  } | null>(null)
  const [nodes, setNodes, onNodesChange] = useNodesState<RuntimeNode>([])
  const [allocationTargetId, setAllocationTargetId] = useState<string | null>(
    null,
  )
  const [workspacePositions, setWorkspacePositions] = useState<
    Record<string, CanvasPoint>
  >(() => readWorkspacePositions(selectedSession))
  const [agentPositions, setAgentPositions] = useState<
    Record<string, CanvasPoint>
  >(readAgentPositions)
  const instance = useRef<ReactFlowInstance<RuntimeNode> | null>(null)
  const showContextMenu = useCallback(
    (
      event: CanvasContextMenuEvent,
      initialScope?: AutomationScope,
      targetLabel?: string,
      automationPlacement?: CanvasPlacement,
    ) => {
      event.preventDefault()
      const point = instance.current?.screenToFlowPosition({
        x: event.clientX,
        y: event.clientY,
      })
      if (!point) return
      const flowElement =
        event.target instanceof Element
          ? event.target.closest('.react-flow')
          : null
      if (!flowElement) return
      const bounds = flowElement.getBoundingClientRect()
      setContextMenu({
        automationPlacement,
        initialScope,
        left: Math.max(
          8,
          Math.min(event.clientX - bounds.left, bounds.width - 202),
        ),
        placement: {
          x: point.x - 58,
          y: point.y - 58,
          width: 116,
          height: 116,
        },
        targetLabel,
        top: Math.max(
          8,
          Math.min(event.clientY - bounds.top, bounds.height - 210),
        ),
      })
    },
    [],
  )
  const handleSelectionChange = useCallback(
    ({ nodes: selectedNodes }: { nodes: RuntimeNode[] }) => {
      if (selectedNodes.length > 0) {
        onSelectionChange(selectionFromNodes(selectedNodes))
      }
    },
    [onSelectionChange],
  )
  const commitNodePosition = useCallback(
    (node: RuntimeNode, position: CanvasPoint) => {
      if (node.type === 'project') {
        const project = (node.data as ProjectNodeData).project
        const geometry = project.placement.geometry
        onProjectPlacementChange(project, {
          x: position.x,
          y: position.y,
          width: node.measured?.width ?? geometry.width,
          height: node.measured?.height ?? geometry.height,
        })
      } else if (node.type === 'automation') {
        const automation = (node.data as AutomationNodeData).automation
        const geometry = automation.placement.geometry
        onAutomationPlacementChange(automation, {
          x: position.x,
          y: position.y,
          width: node.measured?.width ?? geometry.width,
          height: node.measured?.height ?? geometry.height,
        })
      } else if (node.type === 'coordination-node') {
        const coordinationNode = (node.data as CoordinationNodeData).node
        const geometry = coordinationNode.placement.geometry
        onCoordinationNodePlacementChange(coordinationNode, {
          x: position.x,
          y: position.y,
          width: node.measured?.width ?? geometry.width,
          height: node.measured?.height ?? geometry.height,
        })
      } else if (node.type === 'workspace') {
        const workspace = (node.data as WorkspaceNodeData).workspace
        setWorkspacePositions((current) => {
          const next = {
            ...current,
            [workspace.runtime_id]: position,
          }
          writeWorkspacePositions(selectedSession, next)
          return next
        })
      } else {
        setAgentPositions((current) => {
          const next = {
            ...current,
            [node.id]: position,
          }
          writeAgentPositions(next)
          return next
        })
      }
    },
    [
      onCoordinationNodePlacementChange,
      onAutomationPlacementChange,
      onProjectPlacementChange,
      selectedSession,
    ],
  )
  const handleNodesChange = useCallback(
    (changes: NodeChange<RuntimeNode>[]) => {
      onNodesChange(changes)
      for (const change of changes) {
        if (
          change.type !== 'position' ||
          !change.position ||
          change.dragging !== false
        ) {
          continue
        }
        const node = instance.current?.getNode(change.id)
        if (node) commitNodePosition(node, change.position)
      }
    },
    [commitNodePosition, onNodesChange],
  )
  const arrangeSpaces = useCallback(() => {
    const flowInstance = instance.current
    const yardNode = flowInstance?.getNode('yard-orchestrator')
    if (
      !flowInstance ||
      !yardNode ||
      (projects.length === 0 && coordinationNodes.length === 0)
    ) {
      return
    }

    const yardWidth =
      yardNode.measured?.width ??
      numericDimension(yardNode.style?.width, WORKER_NODE_WIDTH)
    const yardHeight =
      yardNode.measured?.height ??
      numericDimension(yardNode.style?.height, WORKER_NODE_HEIGHT)
    const center = {
      x: yardNode.position.x + yardWidth / 2,
      y: yardNode.position.y + yardHeight / 2,
    }
    const projectGeometry = projects
      .map((project) => {
        const node = flowInstance.getNode(`project:${project.id}`)
        if (!node) return null
        const stored = project.placement.geometry
        return {
          height:
            node.measured?.height ??
            numericDimension(node.style?.height, stored.height),
          nodeId: node.id,
          project,
          width:
            node.measured?.width ??
            numericDimension(node.style?.width, stored.width),
        }
      })
      .filter(
        (
          project,
        ): project is {
          height: number
          nodeId: string
          project: Project
          width: number
        } => project !== null,
      )
      .sort(
        (left, right) =>
          left.project.name.localeCompare(right.project.name) ||
          left.project.id.localeCompare(right.project.id),
      )
    const maximumProjectDiagonal =
      projectGeometry.length > 0
        ? Math.max(
            ...projectGeometry.map(({ height, width }) =>
              Math.hypot(width, height),
            ),
          )
        : 0
    const yardDiagonal = Math.hypot(yardWidth, yardHeight)
    const count = Math.max(1, projectGeometry.length)
    const neighborRadius =
      count > 1
        ? (maximumProjectDiagonal + 48) /
          (2 * Math.sin(Math.PI / count))
        : maximumProjectDiagonal + 48
    const radius = Math.max(
      coordinationNodes.length > 0 ? 520 : 280,
      neighborRadius,
      yardDiagonal / 2 + maximumProjectDiagonal / 2 + 72,
    )
    const placements = projectGeometry.map(
      ({ height, nodeId, project, width }, index) => {
        const angle = -Math.PI / 2 + (index * Math.PI * 2) / count
        return {
          nodeId,
          placement: {
            height,
            width,
            x: center.x + Math.cos(angle) * radius - width / 2,
            y: center.y + Math.sin(angle) * radius - height / 2,
          },
          project,
        }
      },
    )
    const coordinationPlacements = coordinationNodes.flatMap((node, index) => {
      const flowNode = flowInstance.getNode(`coordination-node:${node.id}`)
      if (!flowNode) return []
      const width =
        flowNode.measured?.width ??
        numericDimension(
          flowNode.style?.width,
          node.placement.geometry.width,
        )
      const height =
        flowNode.measured?.height ??
        numericDimension(
          flowNode.style?.height,
          node.placement.geometry.height,
        )
      const angle =
        -Math.PI / 2 +
        (index * Math.PI * 2) / Math.max(1, coordinationNodes.length)
      const innerRadius = coordinationNodes.length > 1 ? 220 : 190
      return [
        {
          node,
          nodeId: flowNode.id,
          placement: {
            height,
            width,
            x: center.x + Math.cos(angle) * innerRadius - width / 2,
            y: center.y + Math.sin(angle) * innerRadius - height / 2,
          },
        },
      ]
    })
    const allPlacements = [...placements, ...coordinationPlacements]
    const placementByNodeId = new Map(
      allPlacements.map(({ nodeId, placement }) => [nodeId, placement]),
    )
    setAgentPositions((current) => {
      const next = {
        ...current,
        'yard-orchestrator': { ...yardNode.position },
      }
      writeAgentPositions(next)
      return next
    })
    setNodes((current) =>
      current.map((node) => {
        const placement = placementByNodeId.get(node.id)
        return placement
          ? {
              ...node,
              position: { x: placement.x, y: placement.y },
            }
          : node
      }),
    )
    placements.forEach(({ placement, project }) => {
      onProjectPlacementChange(project, placement)
    })
    coordinationPlacements.forEach(({ node, placement }) => {
      onCoordinationNodePlacementChange(node, placement)
    })

    window.requestAnimationFrame(() => {
      const arrangedIds = new Set([
        'yard-orchestrator',
        ...allPlacements.map(({ nodeId }) => nodeId),
      ])
      void flowInstance.fitView({
        duration: window.matchMedia('(prefers-reduced-motion: reduce)').matches
          ? 0
          : 420,
        maxZoom: 1.05,
        nodes: flowInstance
          .getNodes()
          .filter((node) => arrangedIds.has(node.id)),
        padding: 0.14,
      })
    })
  }, [
    coordinationNodes,
    onCoordinationNodePlacementChange,
    onProjectPlacementChange,
    projects,
    setNodes,
  ])
  const edges = useMemo<Edge[]>(
    () => {
      const allocationEdges = projects.flatMap((project) =>
        assignments
          .filter(
            (assignment) =>
              assignment.project_id === project.id &&
              isVisibleAssignment(assignment, project),
          )
          .map((assignment) => ({
            id: `allocation:${assignment.id}`,
            source: `orchestrator:${project.id}`,
            sourceHandle: 'allocation-source',
            target: `assigned-worker:${assignment.worker.id}`,
            targetHandle: 'allocation-target',
            type: 'smoothstep',
            animated: false,
            selectable: false,
            deletable: false,
            reconnectable: false,
            focusable: false,
            className: 'allocation-edge',
            ariaLabel: `${project.name} orchestrator coordinates ${assignment.profile_name}`,
          })),
      )
      const childEdges = nodes.flatMap((node): Edge[] => {
        if (node.type !== 'child-agent') return []
        const { accent, agent, parentNodeId } =
          node.data as ChildAgentNodeData
        return [
          {
            id: `provider-child-edge:${agent.runtime_id}`,
            source: parentNodeId,
            sourceHandle: 'child-source',
            target: node.id,
            targetHandle: 'child-target',
            type: 'smoothstep',
            animated: false,
            selectable: false,
            deletable: false,
            reconnectable: false,
            focusable: false,
            zIndex: 2,
            style: {
              opacity: 1,
              stroke: accent,
              strokeWidth: 3,
            },
            className: 'provider-child-edge',
            ariaLabel: `${childAgentLabel(agent)} belongs to its parent agent session`,
          },
        ]
      })
      const relationshipEdges = projectRelationships.map(
        (relationship): Edge => ({
          id: `project-relationship:${relationship.id}`,
          source: `project:${relationship.source_project_id}`,
          sourceHandle: 'relationship-source',
          target: `project:${relationship.target_project_id}`,
          targetHandle: 'relationship-target',
          type: 'smoothstep',
          animated: false,
          selectable: false,
          deletable: false,
          reconnectable: false,
          focusable: false,
          zIndex: 1,
          markerEnd: {
            type: MarkerType.ArrowClosed,
            color: 'var(--muted)',
            height: 14,
            width: 14,
          },
          className: 'information-path project-relationship-edge',
          ariaLabel: `${
            projects.find(
              (project) =>
                project.id === relationship.source_project_id,
            )?.name ?? 'Project'
          } depends on ${
            projects.find(
              (project) =>
                project.id === relationship.target_project_id,
            )?.name ?? 'project'
          }`,
        }),
      )
      const latestRouteByProject = new Map<
        string,
        YardOrchestratorRoute
      >()
      for (const route of yardOrchestratorRoutes) {
        const current = latestRouteByProject.get(route.target_project_id)
        if (
          !current ||
          route.updated_at_unix_ms > current.updated_at_unix_ms
        ) {
          latestRouteByProject.set(route.target_project_id, route)
        }
      }
      const coordinationEdges = yardOrchestrator?.worker
        ? projects.map((project): Edge => {
            const route = latestRouteByProject.get(project.id)
            const communicationState = communicationPathState(
              route,
              projectStatusReports[project.id],
            )
            return {
              id: `yard-route:${project.id}`,
              source: 'yard-orchestrator',
              sourceHandle: 'coordination-source',
              target: `project:${project.id}`,
              targetHandle: 'coordination-target',
              type: 'smoothstep',
              animated: communicationState === 'active',
              selectable: false,
              deletable: false,
              reconnectable: false,
              focusable: false,
              zIndex: 1,
              className: `information-path coordination-edge coordination-edge--${communicationState}`,
              data: {
                communicationState,
                status: route?.status ?? 'idle',
              },
              ariaLabel: `Yard connection to ${project.name}, ${communicationState}${
                route ? `, latest route ${route.status}` : ''
              }`,
            }
          })
        : []
      const latestNodeRouteByAttachment = new Map<
        string,
        CoordinationNodeRoute
      >()
      for (const route of coordinationNodeRoutes) {
        const key = `${route.node_id}:${route.target_project_id}`
        const current = latestNodeRouteByAttachment.get(key)
        if (
          !current ||
          route.updated_at_unix_ms > current.updated_at_unix_ms
        ) {
          latestNodeRouteByAttachment.set(key, route)
        }
      }
      const nodeAttachmentEdges = coordinationNodes.flatMap((node) =>
        node.attached_project_ids
          .filter((projectId) =>
            projects.some((project) => project.id === projectId),
          )
          .map((projectId): Edge => {
            const route = latestNodeRouteByAttachment.get(
              `${node.id}:${projectId}`,
            )
            const communicationState = communicationPathState(route)
            return {
              id: `coordination-attachment:${node.id}:${projectId}`,
              source: `coordination-node:${node.id}`,
              sourceHandle: 'node-source',
              target: `project:${projectId}`,
              targetHandle: 'coordination-target',
              type: 'smoothstep',
              animated: communicationState === 'active',
              selectable: false,
              deletable: false,
              reconnectable: false,
              focusable: false,
              zIndex: 1,
              className: `information-path node-attachment-edge node-attachment-edge--${node.kind} coordination-edge--${communicationState}`,
              data: {
                communicationState,
                kind: node.kind,
                status: route?.status ?? 'attached',
              },
              ariaLabel: `${node.name} attached to ${
                projects.find((project) => project.id === projectId)?.name ??
                'project'
              }${route ? `, latest route ${route.status}` : ''}`,
            }
          }),
      )
      const nodeIds = new Set(nodes.map((node) => node.id))
      const automationEdges = automations.flatMap((automation): Edge[] => {
        const target = automationTargetNodeId(automation.scope)
        if (!nodeIds.has(target)) return []
        const latestStatus = automation.latest_run?.status ?? 'never'
        return [
          {
            id: `automation-target:${automation.id}`,
            source: `automation:${automation.id}`,
            sourceHandle: 'automation-source',
            target,
            targetHandle: 'automation-target',
            type: 'smoothstep',
            animated:
              automation.state === 'active' &&
              latestStatus === 'submitted',
            selectable: false,
            deletable: false,
            reconnectable: false,
            focusable: false,
            zIndex: 2,
            className: `automation-target-edge automation-target-edge--${automation.state} automation-target-edge--${latestStatus}`,
            ariaLabel: `${automation.name} targets its orchestrator`,
          },
        ]
      })
      return [
        ...relationshipEdges,
        ...coordinationEdges,
        ...nodeAttachmentEdges,
        ...automationEdges,
        ...allocationEdges,
        ...childEdges,
      ]
    },
    [
      assignments,
      automations,
      coordinationNodes,
      coordinationNodeRoutes,
      nodes,
      projectRelationships,
      projectStatusReports,
      projects,
      yardOrchestrator,
      yardOrchestratorRoutes,
    ],
  )

  const handleConnect = useCallback(
    (connection: Connection) => {
      if (
        connection.source.startsWith('coordination-node:') &&
        connection.target.startsWith('project:')
      ) {
        onCoordinationNodeConnect(
          connection.source.slice('coordination-node:'.length),
          connection.target.slice('project:'.length),
        )
        return
      }
      if (
        !connection.source.startsWith('project:') ||
        !connection.target.startsWith('project:') ||
        connection.source === connection.target
      ) {
        return
      }
      onProjectConnect(
        connection.source.slice('project:'.length),
        connection.target.slice('project:'.length),
      )
    },
    [onCoordinationNodeConnect, onProjectConnect],
  )

  useEffect(() => {
    setWorkspacePositions(readWorkspacePositions(selectedSession))
  }, [selectedSession])

  useEffect(() => {
    setNodes((currentNodes) => {
      const currentById = new Map(
        currentNodes.map((node) => [node.id, node]),
      )
      return buildNodes(
        allocationPayloadByRuntimeId,
        assignments,
        automations,
        coordinationNodes,
        inventory,
        projectAccents,
        projectStatusReports,
        projects,
        runtimeLoading,
        selectedSession,
        visibleWorkers,
        yardOrchestrator,
        allocationTargetId,
        workspacePositions,
        agentPositions,
        onProjectPlacementChange,
      ).map((node) => {
        const current = currentById.get(node.id)
        return current
          ? {
              ...node,
              measured: current.measured,
              ...(current.dragging || current.resizing
                ? {
                    position: current.position,
                    style: current.style,
                  }
                : {}),
              selected: current.selected,
            }
          : node
      })
    })
  }, [
    allocationTargetId,
    allocationPayloadByRuntimeId,
    assignments,
    automations,
    coordinationNodes,
    inventory,
    onProjectPlacementChange,
    projectAccents,
    projects,
    projectStatusReports,
    runtimeLoading,
    selectedSession,
    setNodes,
    visibleWorkers,
    workspacePositions,
    yardOrchestrator,
    agentPositions,
  ])

  return (
    <ReactFlow
      aria-label="Yard project canvas"
      colorMode={theme}
      defaultViewport={{ x: 44, y: 42, zoom: 0.88 }}
      deleteKeyCode={null}
      maxZoom={1.6}
      minZoom={0.3}
      edges={edges}
      edgesFocusable={false}
      edgesReconnectable={false}
      elevateNodesOnSelect={false}
      multiSelectionKeyCode="Shift"
      nodeTypes={NODE_TYPES}
      nodes={nodes}
      nodesConnectable
      onConnect={handleConnect}
      onPaneClick={() => setContextMenu(null)}
      onPaneContextMenu={(event) => showContextMenu(event)}
      onNodeContextMenu={(event, node) => {
        if (node.type === 'yard-orchestrator') {
          showContextMenu(
            event,
            { kind: 'yard_orchestrator' },
            'Superintendent',
            {
              x: node.position.x + WORKER_NODE_WIDTH + 24,
              y:
                node.position.y +
                (WORKER_NODE_HEIGHT - AUTOMATION_NODE_HEIGHT) / 2,
              width: AUTOMATION_NODE_WIDTH,
              height: AUTOMATION_NODE_HEIGHT,
            },
          )
          return
        }
        if (node.type === 'orchestrator') {
          const project = (node.data as OrchestratorNodeData).project
          showContextMenu(
            event,
            {
              kind: 'project_orchestrator',
              project_id: project.id,
            },
            `${project.name} orchestrator`,
            {
              x:
                project.placement.geometry.x +
                node.position.x +
                (WORKER_NODE_WIDTH - AUTOMATION_NODE_WIDTH) / 2,
              y:
                Math.max(
                  8,
                  project.placement.geometry.y -
                    AUTOMATION_NODE_HEIGHT -
                    18,
                ),
              width: AUTOMATION_NODE_WIDTH,
              height: AUTOMATION_NODE_HEIGHT,
            },
          )
          return
        }
        if (node.type === 'coordination-node') {
          const coordinationNode = (node.data as CoordinationNodeData).node
          if (coordinationNode.kind !== 'workstream') return
          showContextMenu(
            event,
            {
              kind: 'workstream_coordination_node',
              node_id: coordinationNode.id,
            },
            coordinationNode.name,
            {
              x:
                coordinationNode.placement.geometry.x +
                coordinationNode.placement.geometry.width +
                24,
              y:
                coordinationNode.placement.geometry.y +
                (coordinationNode.placement.geometry.height -
                  AUTOMATION_NODE_HEIGHT) /
                  2,
              width: AUTOMATION_NODE_WIDTH,
              height: AUTOMATION_NODE_HEIGHT,
            },
          )
        }
      }}
      onNodeClick={(event, node) => {
        if (!event.shiftKey) {
          onSelectionChange(selectionFromNodes([node]))
        }
      }}
      onSelectionChange={handleSelectionChange}
      onNodesChange={handleNodesChange}
      onDragLeave={() => setAllocationTargetId(null)}
      onDragOver={(event) => {
        if (!event.dataTransfer.types.includes(ALLOCATION_DRAG_TYPE)) return
        event.preventDefault()
        event.dataTransfer.dropEffect =
          event.dataTransfer.effectAllowed === 'move' ? 'move' : 'copy'
        const point = instance.current?.screenToFlowPosition({
          x: event.clientX,
          y: event.clientY,
        })
        if (!point) return
        const target = projectAtPoint(
          instance.current?.getNodes() ?? [],
          point,
        )
        setAllocationTargetId(target?.id ?? null)
      }}
      onDrop={(event) => {
        event.preventDefault()
        const payload = getAllocationDragData(event.dataTransfer)
        const point = instance.current?.screenToFlowPosition({
          x: event.clientX,
          y: event.clientY,
        })
        const projectId = point
          ? projectAtPoint(instance.current?.getNodes() ?? [], point)?.id
          : allocationTargetId
        setAllocationTargetId(null)
        if (payload && projectId) onAllocationDrop(payload, projectId)
      }}
      onInit={(flowInstance) => {
        instance.current = flowInstance
        const compact = window.matchMedia('(max-width: 680px)').matches
        void flowInstance.setViewport(
          compact
            ? { x: 18, y: 34, zoom: 0.72 }
            : { x: 44, y: 42, zoom: 0.88 },
        )
      }}
      proOptions={{ hideAttribution: true }}
    >
      <Panel className="canvas-tools-panel" position="top-left">
        <button
          aria-label="Project pulse"
          onClick={(event) => onOpenProjectPulse(event.currentTarget)}
          title="Open Project pulse"
          type="button"
        >
          <Activity aria-hidden="true" size={15} />
          <span>Project pulse</span>
        </button>
        <button
          aria-label="Arrange spaces"
          disabled={
            !yardOrchestrator ||
            (projects.length === 0 && coordinationNodes.length === 0)
          }
          onClick={arrangeSpaces}
          title="Arrange project spaces around Yard"
          type="button"
        >
          <Orbit aria-hidden="true" size={15} />
          <span>Arrange spaces</span>
        </button>
      </Panel>
      {contextMenu ? (
        <div
          aria-label="Create map node"
          className="map-context-menu"
          onContextMenu={(event) => event.preventDefault()}
          role="menu"
          style={{ left: contextMenu.left, top: contextMenu.top }}
        >
          <button
            className="map-context-menu__automation"
            onClick={() => {
              const centerX =
                contextMenu.placement.x +
                contextMenu.placement.width / 2
              const centerY =
                contextMenu.placement.y +
                contextMenu.placement.height / 2
              onCreateAutomation(
                contextMenu.automationPlacement ?? {
                  x: centerX - AUTOMATION_NODE_WIDTH / 2,
                  y: centerY - AUTOMATION_NODE_HEIGHT / 2,
                  width: AUTOMATION_NODE_WIDTH,
                  height: AUTOMATION_NODE_HEIGHT,
                },
                contextMenu.initialScope,
              )
              setContextMenu(null)
            }}
            role="menuitem"
            type="button"
          >
            <Clock3 aria-hidden="true" size={16} />
            <span>
              <strong>Automation</strong>
              <small>
                {contextMenu.targetLabel
                  ? `Target ${contextMenu.targetLabel}`
                  : 'Schedule an orchestrator'}
              </small>
            </span>
          </button>
          <button
            onClick={() => {
              onCreateCoordinationNode(
                contextMenu.placement,
                'workstream',
              )
              setContextMenu(null)
            }}
            role="menuitem"
            type="button"
          >
            <Network aria-hidden="true" size={16} />
            <span>
              <strong>Workstream</strong>
              <small>Coordinate projects</small>
            </span>
          </button>
          <button
            onClick={() => {
              onCreateCoordinationNode(
                contextMenu.placement,
                'knowledge_store',
              )
              setContextMenu(null)
            }}
            role="menuitem"
            type="button"
          >
            <Database aria-hidden="true" size={16} />
            <span>
              <strong>Knowledge store</strong>
              <small>Collect project context</small>
            </span>
          </button>
        </div>
      ) : null}
      <MiniMap
        ariaLabel="Project canvas map"
        maskColor={
          theme === 'dark'
            ? 'rgba(17, 23, 21, 0.76)'
            : 'rgba(229, 233, 231, 0.76)'
        }
        nodeColor={(node) => {
          if (node.type === 'automation') return '#3978b8'
          if (node.type === 'yard-orchestrator') return '#c64b3c'
          if (node.type === 'coordination-node') {
            return (node.data as CoordinationNodeData).node.kind ===
              'knowledge_store'
              ? '#347f78'
              : '#d99832'
          }
          if (node.type !== 'project') return '#d94a37'
          return (node.data as ProjectNodeData).accent
        }}
        pannable
        zoomable
      />
      <Controls position="bottom-right" showInteractive={false} />
    </ReactFlow>
  )
}

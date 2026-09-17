import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
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
  ViewportPortal,
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
import {
  OrchestratorSprite,
  SubagentSprite,
  SuperintendentSprite,
  WorkerSprite,
} from './crewArt'
import type {
  Assignment,
  Automation,
  AutomationScope,
  CanvasPlacement,
  CoordinationNode,
  CoordinationNodeKind,
  CoordinationNodeRoute,
  ManagedRuntimeWorkspace,
  ManagedRuntimeWorkspaceKind,
  ObservedChildAgent,
  ObservedStatus,
  ObservedWorker,
  Project,
  ProjectRelationship,
  ProviderSessionRef,
  RuntimeInventory,
  RuntimeTopology,
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
import {
  mergeBounds,
  projectBounds,
  projectDelta,
  projectPoint,
  unprojectDelta,
  unprojectPoint,
  type WorldPoint,
} from './mapProjection'
import { ProjectedMap } from './ProjectedMap'
import { createLatestFrameQueue } from './latestFrameQueue'
import {
  EMPTY_SCENE,
  stableHash,
  territoryBuildings,
  type ProjectedAnchor,
  type ProjectedAnchorKind,
  type ProjectedRoute,
  type ProjectedRouteKind,
  type ProjectedRouteState,
  type ProjectedScene,
  type ProjectedTerritory,
} from './mapScene'
import type { ThemeDefinition } from './theme'
import type { MapVisualMode } from './mapVisualMode'

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
  runtimeTopology: RuntimeTopology | null
  selectedSession: string
  theme: ThemeDefinition
  visualMode: MapVisualMode
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
  projectTokenTotal: number
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
  managedKind?: ManagedRuntimeWorkspaceKind
  managedLabel?: string
  managedOccupantSummary?: string
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
const TERRITORY_BOX_PAD = 24
/**
 * Billboards are drawn smaller than the flat plan cards. The ground plane
 * compresses world distances (0.82 across x, 0.44 across y), so a full-size
 * card would both tower over its territory and collide with its neighbours once
 * the grid they sit on is projected. The value has to stay below the tightest
 * projected gap in the agent layouts — the child-agent tree column, which
 * projects a 113x11.5 world step to roughly 83 screen units — so that a child
 * still stands clear of its parent.
 */
const BILLBOARD_SCALE = 0.78
const BILLBOARD_ZOOM_COMPENSATION_START = 0.72
const BILLBOARD_ZOOM_COMPENSATION_MAX = 1.8
/** Vertical room the tallest structures need above their ground footprint. */
const BUILDING_HEADROOM = 140

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

type ProjectedNodeProperties = CSSProperties & {
  '--billboard-height'?: string
  '--billboard-scale'?: string
  '--billboard-width'?: string
  '--territory-box-height'?: string
  '--territory-box-left'?: string
  '--territory-box-top'?: string
  '--territory-box-width'?: string
  '--territory-label-x'?: string
  '--territory-label-y'?: string
}

type RuntimeCanvasProperties = CSSProperties & {
  '--billboard-zoom-compensation': string
}

type BuildingProperties = CSSProperties & {
  '--building-depth': string
  '--building-height': string
  '--building-width': string
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
      '--building-depth': `${5 + ((hash >>> 12) % 4)}px`,
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

/**
 * Absolute (root-relative) world position of every node. ReactFlow stores child
 * node positions relative to their parent, but the projection is defined on
 * absolute world coordinates, so the offsets have to be resolved first.
 */
function absoluteNodePositions(nodes: RuntimeNode[]) {
  const nodeById = new Map(nodes.map((node) => [node.id, node]))
  const resolved = new Map<string, CanvasPoint>()

  const resolve = (node: RuntimeNode, seen: Set<string>): CanvasPoint => {
    const cached = resolved.get(node.id)
    if (cached) return cached
    let position = { x: node.position.x, y: node.position.y }
    const parentId = node.parentId
    if (parentId && !seen.has(parentId)) {
      const parent = nodeById.get(parentId)
      if (parent) {
        const parentPosition = resolve(parent, new Set(seen).add(node.id))
        position = {
          x: position.x + parentPosition.x,
          y: position.y + parentPosition.y,
        }
      }
    }
    resolved.set(node.id, position)
    return position
  }

  for (const node of nodes) resolve(node, new Set([node.id]))
  return resolved
}

const ANCHOR_KINDS: Partial<Record<RuntimeNode['type'], ProjectedAnchorKind>> = {
  'assigned-worker': 'assigned-worker',
  automation: 'automation',
  'child-agent': 'child-agent',
  'coordination-node': 'coordination-node',
  orchestrator: 'orchestrator',
  worker: 'worker',
  'yard-orchestrator': 'yard-orchestrator',
}

const ROUTE_KIND_BY_PREFIX: [string, ProjectedRouteKind][] = [
  ['project-relationship:', 'relationship'],
  ['yard-route:', 'coordination'],
  ['coordination-attachment:', 'coordination'],
  ['automation-target:', 'coordination'],
  ['allocation:', 'allocation'],
  ['provider-child-edge:', 'child'],
]

function routeKind(edgeId: string): ProjectedRouteKind {
  return (
    ROUTE_KIND_BY_PREFIX.find(([prefix]) => edgeId.startsWith(prefix))?.[1] ??
    'coordination'
  )
}

function routeState(edge: Edge): ProjectedRouteState {
  const state = (edge.data as { communicationState?: unknown } | undefined)
    ?.communicationState
  return state === 'active' || state === 'failed' ? state : 'idle'
}

function nodeDimensions(node: RuntimeNode, fallbackWidth = 0, fallbackHeight = 0) {
  return {
    width:
      node.measured?.width ??
      numericDimension(node.style?.width, fallbackWidth),
    height:
      node.measured?.height ??
      numericDimension(node.style?.height, fallbackHeight),
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
      {/*
        Deliberate 2.5D compromise, not an oversight: NodeResizer computes its
        eight handles from the node's own measured flat rectangle, and there is
        no supported way to move them onto a projected parallelogram's corners
        without forking the component. Resizing therefore keeps operating on the
        flat box; the projected territory re-renders from the new width/height
        when onResizeEnd fires, so the shape settles on release rather than
        tracking the drag. Projected corner handles need per-corner delta maths
        beyond the translation-only inverse projection and are explicit future
        scope. Do not "fix" this by hiding the resizer in 2.5D.
      */}
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
      {/*
        Deliberate 2.5D compromise, not an oversight: these handles stay live
        and keep their flat position relative to the node box. ReactFlow's
        connection-line renderer draws in flat node space and cannot be
        reprojected without replacing the whole gesture, so connect-by-drag
        remains a flat interaction over a projected territory. Making the
        handles pointer-events:none would silently kill relationship and
        coordination creation with no test catching it.
      */}
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
  const {
    workspace,
    workerCount,
    childAgentCount,
    managedKind,
    managedLabel,
    managedOccupantSummary,
  } = data
  const label = managedLabel ?? workspace.label
  const workspaceType =
    managedKind === 'yard_central'
      ? 'Yard central territory'
      : managedKind === 'coordination'
        ? 'Yard coordination territory'
        : managedKind === 'provisioning'
          ? 'Runtime provisioning reservation'
          : managedKind === 'quarantined'
            ? 'Quarantined runtime reservation'
        : managedKind === 'cleanup_pending'
          ? 'Managed cleanup territory'
          : 'Herdr workspace'
  return (
    <div
      className={`workspace-region runtime-workspace-region ${selected ? 'is-selected' : ''}`}
      data-managed-kind={managedKind}
      data-status={workspace.status}
    >
      <div className="workspace-region__heading">
        <div className="workspace-region__title">
          <span className="workspace-region__index">
            {managedKind === 'yard_central' ? 'C' : 'H'}
          </span>
          <strong>{label}</strong>
        </div>
        <span className="workspace-region__count">
          {workerCount} agent{workerCount === 1 ? '' : 's'}
          {childAgentCount > 0
            ? ` + ${childAgentCount} child${childAgentCount === 1 ? '' : 'ren'}`
            : ''}
          {managedOccupantSummary ? ` / ${managedOccupantSummary}` : ''}
        </span>
      </div>
      <div className="workspace-region__meta">
        <span>{workspaceType}</span>
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
  const activity =
    worker.status === 'blocked'
      ? attentionActivity('Blocked: needs input')
      : worker.status === 'done'
        ? quietActivity('Ready for review')
        : null

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
        <WorkerActivityBubble activity={activity} />
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
              <span aria-hidden="true" className="worker-marker__sprite">
                <WorkerSprite />
              </span>
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
  const activity =
    agent.status === 'blocked'
      ? attentionActivity('Blocked: needs input')
      : agent.status === 'done'
        ? quietActivity('Ready for review')
        : agent.status === 'working'
          ? activeActivity(agent.description ?? agent.role ?? '')
          : null

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
      <WorkerActivityBubble activity={activity} />
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
        <span
          aria-hidden="true"
          className="worker-marker__sprite worker-marker__sprite--child"
        >
          <SubagentSprite />
        </span>
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

type WorkerActivityState = 'active' | 'attention' | 'quiet'

interface WorkerActivity {
  state: WorkerActivityState
  text: string
}

function activeActivity(text: string): WorkerActivity | null {
  return text.trim() ? { state: 'active', text: text.trim() } : null
}

function attentionActivity(text: string): WorkerActivity {
  return { state: 'attention', text }
}

function quietActivity(text: string): WorkerActivity {
  return { state: 'quiet', text }
}

function runtimeActivity(
  status: ObservedStatus,
  processState: WorkerRuntimeBinding['process_state'] | undefined,
  activeText: string,
): WorkerActivity {
  if (processState === 'exited') {
    return attentionActivity(
      status === 'blocked' ? 'Blocked: session ended' : 'Session ended',
    )
  }
  if (status === 'blocked') return attentionActivity('Blocked: needs input')
  if (status === 'unknown') return quietActivity(activeText)
  if (status === 'done') return quietActivity('Ready for review')
  if (status === 'idle') return quietActivity('Waiting for direction')
  return activeActivity(activeText) ?? quietActivity('Working')
}

function WorkerActivityBubble({
  activity,
}: {
  activity: WorkerActivity | null
}) {
  if (!activity) return null
  return (
    <span
      aria-hidden="true"
      className="worker-activity-bubble"
      data-activity-state={activity.state}
      title={activity.text}
    >
      <span className="worker-activity-bubble__signal" />
      <span className="worker-activity-bubble__text">{activity.text}</span>
    </span>
  )
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
  const activity =
    configured &&
    (runtimeState.processState === 'exited' ||
      runtimeState.status === 'blocked' ||
      runtimeState.status === 'done' ||
      runtimeState.status === 'working')
    ? runtimeActivity(
        runtimeState.status,
        runtimeState.processState,
        'Coordinating Yard',
      )
    : null

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
        <WorkerActivityBubble activity={activity} />
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
          <span aria-hidden="true" className="worker-marker__sprite">
            <SuperintendentSprite />
          </span>
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
  const activity =
    statusReport?.state === 'needs_attention'
      ? attentionActivity(
          statusReport.blockers[0] ||
            statusReport.next ||
            'Blocked: needs input',
        )
      : statusReport ||
          runtimeState.processState === 'exited' ||
          runtimeState.status === 'blocked' ||
          runtimeState.status === 'done' ||
          runtimeState.status === 'working'
        ? runtimeActivity(
            runtimeState.status,
            runtimeState.processState,
            statusReport?.next ||
              (statusReport?.state === 'idle'
                ? 'Ready for direction'
                : `Coordinating ${project.name}`),
          )
        : null

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
        <WorkerActivityBubble activity={activity} />
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
              <span aria-hidden="true" className="worker-marker__sprite">
                <OrchestratorSprite />
              </span>
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
  const activity = runtimeActivity(
    runtimeState.status,
    runtimeState.processState,
    assignment.lifecycle === 'allocating'
      ? `Starting: ${assignment.objective}`
      : assignment.lifecycle === 'handing_off'
        ? `Handing off: ${assignment.objective}`
        : assignment.objective,
  )

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
        <WorkerActivityBubble activity={activity} />
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
              <span aria-hidden="true" className="worker-marker__sprite">
                <WorkerSprite />
              </span>
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

function managedWorkspaceObservation(
  managed: ManagedRuntimeWorkspace,
  inventory: RuntimeInventory | null,
  order: number,
): WorkspaceObservation {
  const observed = inventory?.workspaces.find(
    (workspace) => workspace.runtime_id === managed.workspace_id,
  )
  if (observed) {
    return { ...observed, label: managed.label }
  }
  return {
    runtime_id: managed.workspace_id,
    order,
    label: managed.label,
    focused: false,
    active_tab_id: '',
    pane_count: 0,
    tab_count: 0,
    status: 'unknown',
    tokens: {},
    worktree: null,
  }
}

function managedWorkspaceOccupantSummary(
  workspace: ManagedRuntimeWorkspace,
): string {
  const count = (kind: ManagedRuntimeWorkspace['occupants'][number]['kind']) =>
    workspace.occupants.filter((occupant) => occupant.kind === kind).length
  const cleanupPending = count('cleanup_pending')
  const provisioning = count('provisioning')
  const quarantined = count('quarantined')
  return [
    cleanupPending > 0 ? `${cleanupPending} cleanup pending` : '',
    provisioning > 0 ? `${provisioning} provisioning` : '',
    quarantined > 0 ? `${quarantined} quarantined` : '',
  ]
    .filter(Boolean)
    .join(' / ')
}

function terminalIdentity(
  adapter: string,
  session: string,
  terminalId: string,
) {
  return JSON.stringify([adapter, session, terminalId])
}

function runtimeTerminalIdentity(
  runtime: WorkerRuntimeBinding | null | undefined,
) {
  return runtime
    ? terminalIdentity(runtime.adapter, runtime.session, runtime.terminal_id)
    : null
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
    inventory.session !== project.runtime.session ||
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

/**
 * Resolve the project territory under a flow-space point.
 *
 * The projection is a linear affine map, so unprojecting the pointer into world
 * space and testing the stored world rectangle is exactly equivalent to
 * projecting every territory into a screen-space parallelogram and running
 * point-in-polygon maths — with none of the fill-rule code. The territory the
 * user sees is a true projected polygon; only the hit test gets to be simple.
 */
function projectAtPoint(
  nodes: RuntimeNode[],
  flowPoint: CanvasPoint,
  visualMode: MapVisualMode = 'flat',
) {
  const point =
    visualMode === 'depth' ? unprojectPoint(flowPoint) : flowPoint
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
  const runtime = assignment.worker.runtime
  return (
    (assignment.lifecycle === 'allocating' ||
      assignment.lifecycle === 'active' ||
      assignment.lifecycle === 'handing_off') &&
    assignment.worker.id !== project.orchestrator.id &&
    (!runtime ||
      runtimeTerminalIdentity(runtime) !==
        runtimeTerminalIdentity(project.orchestrator.runtime))
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
  theme: ThemeDefinition,
  projectAccents: Record<string, string>,
  projectStatusReports: ProjectStatusReports,
  projects: Project[],
  runtimeLoading: boolean,
  runtimeTopology: RuntimeTopology | null,
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
  const activeTopology =
    runtimeTopology?.session === selectedSession ? runtimeTopology : null
  const managedWorkspaces = activeTopology?.managed_workspaces ?? []
  const managedWorkspaceIds = new Set(
    managedWorkspaces.map((workspace) => workspace.workspace_id),
  )
  const managedTerminalIdentities = new Set(
    managedWorkspaces.flatMap((workspace) =>
      workspace.occupants.map((occupant) =>
        terminalIdentity(
          activeTopology?.adapter ?? 'herdr',
          activeTopology?.session ?? selectedSession,
          occupant.terminal_id,
        ),
      ),
    ),
  )
  const yardOrchestratorTerminalIdentity = runtimeTerminalIdentity(
    yardOrchestrator?.worker?.runtime,
  )
  const coordinationTerminalIdentities = coordinationNodes
    .map((node) => runtimeTerminalIdentity(node.worker?.runtime))
    .filter((identity): identity is string => identity !== null)
  const projectNodes = projects.flatMap((project): RuntimeNode[] => {
    const projectWorkspaceInfrastructureManaged = managedWorkspaces.some(
      (managed) =>
        (managed.kind === 'yard_central' ||
          managed.kind === 'coordination') &&
        activeTopology?.adapter === project.runtime.adapter &&
        activeTopology.session === project.runtime.session &&
        managed.workspace_id === project.runtime.workspace_id,
    )
    const workspace = projectWorkspaceInfrastructureManaged
      ? null
      : observedWorkspace(project, inventory)
    const runtimePending =
      runtimeLoading &&
      (!selectedSession || project.runtime.session === selectedSession)
    const allProjectAssignments = assignments.filter(
      (assignment) => assignment.project_id === project.id,
    )
    const activeVisibleAssignments = allProjectAssignments.filter(
      (assignment) => isVisibleAssignment(assignment, project),
    )
    const assignedTerminalIdentities = new Set(
      activeVisibleAssignments
        .map((assignment) =>
          runtimeTerminalIdentity(assignment.worker.runtime),
        )
        .filter((identity): identity is string => identity !== null),
    )
    const orchestratorTerminalIdentity = runtimeTerminalIdentity(
      project.orchestrator.runtime,
    )
    if (orchestratorTerminalIdentity) {
      assignedTerminalIdentities.add(orchestratorTerminalIdentity)
    }
    if (yardOrchestratorTerminalIdentity) {
      assignedTerminalIdentities.add(yardOrchestratorTerminalIdentity)
    }
    managedTerminalIdentities.forEach((identity) =>
      assignedTerminalIdentities.add(identity),
    )
    const workers = workspace && inventory
      ? visibleWorkers.filter(
          (worker) =>
            worker.workspace_id === workspace.runtime_id &&
            !assignedTerminalIdentities.has(
              terminalIdentity(
                inventory.adapter,
                inventory.session,
                worker.terminal_id,
              ),
            ),
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
      ...activeVisibleAssignments.map((assignment) => {
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
      1 + workers.length + activeVisibleAssignments.length
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
    // Real observed usage, not a simulated figure: summed from each assigned
    // worker's own Herdr-reported token counts, the same `tokens` field
    // already on ObservedWorker but never surfaced anywhere in the UI before.
    const projectTokenTotal = allProjectAssignments.reduce(
      (total, assignment) => {
        const worker = observedWorker(
          project,
          assignment.worker.runtime,
          inventory,
        )
        if (!worker) return total
        const workerTotal = Object.values(worker.tokens).reduce(
          (sum, value) => sum + (Number.parseInt(value, 10) || 0),
          0,
        )
        return total + workerTotal
      },
      0,
    )
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
    const accent = projectAccents[project.id] ?? theme.tokens.accentPrimary
    const projectNode: ProjectNode = {
      id: projectNodeId,
      type: 'project',
      position: { x: geometry.x, y: geometry.y },
      data: {
        project,
        accent,
        buildingCount,
        completedBuildingCount,
        projectTokenTotal,
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
    const assignedNodes: AssignedWorkerNode[] = activeVisibleAssignments.map(
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
  const representedTerminalIdentities = new Set(
    [
      yardOrchestratorTerminalIdentity,
      ...coordinationTerminalIdentities,
      ...managedTerminalIdentities,
      ...projects
        .filter(
          (project) =>
            inventory &&
            project.runtime.adapter === inventory.adapter &&
            project.runtime.session === inventory.session,
        )
        .map((project) =>
          runtimeTerminalIdentity(project.orchestrator.runtime),
        ),
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
        .map((assignment) =>
          runtimeTerminalIdentity(assignment.worker.runtime),
        ),
    ].filter((identity): identity is string => identity !== null),
  )
  const unboundWorkers =
    inventory?.session === selectedSession
      ? visibleWorkers.filter(
          (worker) =>
            !representedWorkspaceIds.has(worker.workspace_id) &&
            !managedWorkspaceIds.has(worker.workspace_id) &&
            !representedTerminalIdentities.has(
              terminalIdentity(
                inventory.adapter,
                inventory.session,
                worker.terminal_id,
              ),
            ),
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
            !managedWorkspaceIds.has(workspace.runtime_id) &&
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
        data: { agent, parentNodeId, accent: theme.tokens.statusInfo },
        ariaLabel: `${childAgentLabel(agent)}, ${agent.provider} child agent, ${agent.status}`,
        focusable: true,
      })
    })
    columnBottoms[column] += height + 58
  })

  const centralManagedWorkspace = managedWorkspaces.find(
    (workspace) => workspace.kind === 'yard_central',
  )
  const renderedManagedWorkspaces = managedWorkspaces.filter(
    (workspace) =>
      workspace.kind !== 'coordination' &&
      (workspace.kind === 'yard_central' ||
        !representedWorkspaceIds.has(workspace.workspace_id)),
  )
  const managedWorkspaceNodes: RuntimeNode[] = []
  const minimumProjectX =
    projects.length > 0
      ? Math.min(...projects.map((project) => project.placement.geometry.x))
      : 440
  const minimumProjectY =
    projects.length > 0
      ? Math.min(...projects.map((project) => project.placement.geometry.y))
      : 88
  const managedWorkspaceStartY = Math.max(...columnBottoms) + 58
  let nonCentralWorkspaceIndex = 0
  renderedManagedWorkspaces.forEach((managed, managedIndex) => {
    const workspace = managedWorkspaceObservation(
      managed,
      inventory,
      managedIndex,
    )
    const includeYardOrchestrator =
      managed.kind === 'yard_central' && yardOrchestrator !== null
    const yardWorker = includeYardOrchestrator
      ? yardOrchestrator.worker
      : null
    const yardObserved = includeYardOrchestrator
      ? observedRuntimeWorker(yardWorker?.runtime ?? null, inventory)
      : null
    const roots: ChildAgentRoot[] = [
      ...(includeYardOrchestrator
        ? [
            {
              nodeId: 'yard-orchestrator',
              session:
                yardObserved?.provider_session ??
                yardWorker?.runtime?.provider_session ??
                null,
            },
          ]
        : []),
    ]
    const linkedChildren = linkedChildAgents(inventory, roots)
    const treeLayout = layoutAgentTrees(
      roots,
      linkedChildren,
      agentPositions,
    )
    const visibleAgentCount = roots.length + linkedChildren.length
    const width = Math.max(350, treeLayout.minimumWidth)
    const height = Math.max(
      projectHeight(visibleAgentCount),
      treeLayout.minimumHeight,
    )
    const workspaceNodeId = `workspace:${managed.workspace_id}`
    const defaultPosition =
      managed.kind === 'yard_central'
        ? {
            x: minimumProjectX - width - 48,
            y: Math.min(220, Math.max(48, minimumProjectY - 40)),
          }
        : {
            x: 32 + (nonCentralWorkspaceIndex % 3) * 430,
            y:
              managedWorkspaceStartY +
              Math.floor(nonCentralWorkspaceIndex / 3) * (height + 58),
          }
    if (managed.kind !== 'yard_central') {
      nonCentralWorkspaceIndex += 1
    }
    managedWorkspaceNodes.push({
      id: workspaceNodeId,
      type: 'workspace',
      position:
        workspacePositions[managed.workspace_id] ?? defaultPosition,
      draggable: true,
      dragHandle: '.workspace-region__heading',
      style: { width, height },
      deletable: false,
      data: {
        workspace,
        workerCount: roots.length,
        childAgentCount: linkedChildren.length,
        managedKind: managed.kind,
        managedLabel: managed.label,
        managedOccupantSummary:
          managedWorkspaceOccupantSummary(managed),
      },
      ariaLabel: `${managed.label}, managed ${managed.kind.replaceAll('_', ' ')}${managed.occupants.length > 0 ? `, ${managedWorkspaceOccupantSummary(managed)}` : ''}`,
      focusable: true,
    })
    if (includeYardOrchestrator && yardOrchestrator) {
      const runtimeState = resolvedRuntimeState(
        yardWorker?.runtime ?? null,
        yardObserved,
      )
      managedWorkspaceNodes.push({
        id: 'yard-orchestrator',
        type: 'yard-orchestrator',
        parentId: workspaceNodeId,
        extent: 'parent',
        expandParent: false,
        position:
          treeLayout.positions.get('yard-orchestrator') ??
          workerPosition(0),
        draggable: true,
        dragHandle: '.yard-hub-node__placement-handle',
        style: {
          height: WORKER_NODE_HEIGHT,
          width: WORKER_NODE_WIDTH,
          zIndex: 8,
        },
        deletable: false,
        data: {
          observed: yardObserved,
          orchestrator: yardOrchestrator,
        },
        ariaLabel: yardWorker
          ? `Superintendent, ${runtimeState.status}, process ${runtimeState.processState}`
          : 'Superintendent, not configured',
        focusable: true,
      } satisfies YardOrchestratorNode)
    }
    linkedChildren.forEach(({ agent, parentNodeId }, childIndex) => {
      const childId = childNodeId(agent)
      managedWorkspaceNodes.push({
        id: childId,
        type: 'child-agent',
        parentId: workspaceNodeId,
        extent: 'parent',
        expandParent: false,
        draggable: false,
        position:
          treeLayout.positions.get(childId) ??
          workerPosition(roots.length + childIndex),
        style: {
          height: CHILD_NODE_HEIGHT,
          width: CHILD_NODE_WIDTH,
          zIndex: 7,
        },
        deletable: false,
        data: {
          accent:
            managed.kind === 'yard_central' ? theme.tokens.statusDanger : theme.tokens.statusInfo,
          agent,
          parentNodeId,
        },
        ariaLabel: `${childAgentLabel(agent)}, ${agent.provider} child agent, ${agent.status}`,
        focusable: true,
      } satisfies ChildAgentNode)
    })
  })

  const yardNodes: RuntimeNode[] = []
  if (yardOrchestrator && !centralManagedWorkspace) {
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
          accent: theme.tokens.statusDanger,
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
    ...managedWorkspaceNodes,
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
  runtimeTopology,
  selectedSession,
  theme,
  visualMode,
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
  const [viewportZoom, setViewportZoom] = useState(0.88)
  const instance = useRef<ReactFlowInstance<RuntimeNode> | null>(null)
  const dragOrigins = useRef(new Map<string, CanvasPoint>())
  const nodesById = useRef(new Map<string, RuntimeNode>())
  const framedProjection = useRef(false)
  const showContextMenu = useCallback(
    (
      event: CanvasContextMenuEvent,
      initialScope?: AutomationScope,
      targetLabel?: string,
      automationPlacement?: CanvasPlacement,
    ) => {
      event.preventDefault()
      const flowPoint = instance.current?.screenToFlowPosition({
        x: event.clientX,
        y: event.clientY,
      })
      if (!flowPoint) return
      // Right-click creation stores an unprojected world placement, so the new
      // node lands where the pointer visibly was on the projected ground.
      const point =
        visualMode === 'depth' ? unprojectPoint(flowPoint) : flowPoint
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
    [visualMode],
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
      // In 2.5D a billboard's measured box is its drawn size, not the world
      // footprint it stands on, so persisted geometry must come from the stored
      // placement instead. Persisted values stay unprojected, unscaled world
      // coordinates in both modes.
      const persistedSize = (geometry: CanvasPlacement) =>
        visualMode === 'depth'
          ? { width: geometry.width, height: geometry.height }
          : {
              width: node.measured?.width ?? geometry.width,
              height: node.measured?.height ?? geometry.height,
            }
      if (node.type === 'project') {
        const project = (node.data as ProjectNodeData).project
        const geometry = project.placement.geometry
        onProjectPlacementChange(project, {
          x: position.x,
          y: position.y,
          ...persistedSize(geometry),
        })
      } else if (node.type === 'automation') {
        const automation = (node.data as AutomationNodeData).automation
        const geometry = automation.placement.geometry
        onAutomationPlacementChange(automation, {
          x: position.x,
          y: position.y,
          ...persistedSize(geometry),
        })
      } else if (node.type === 'coordination-node') {
        const coordinationNode = (node.data as CoordinationNodeData).node
        const geometry = coordinationNode.placement.geometry
        onCoordinationNodePlacementChange(coordinationNode, {
          x: position.x,
          y: position.y,
          ...persistedSize(geometry),
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
      visualMode,
    ],
  )
  const handleNodesChange = useCallback(
    (changes: NodeChange<RuntimeNode>[]) => {
      // In 2.5D mode a pointer drag is a movement across the projected ground
      // plane, not across the flat world axes. ReactFlow hands us
      // `dragStartPosition + screenDelta / zoom`; re-deriving that delta and
      // running it back through the inverse projection is what makes a
      // horizontal screen drag change both world axes. Only pointer drags are
      // remapped: keyboard arrow movement stays on the world axes so nudging a
      // project remains a predictable, quantised step.
      const projected = changes.map((change) => {
        if (
          visualMode !== 'depth' ||
          change.type !== 'position' ||
          !change.position
        ) {
          return change
        }
        // The origin is captured lazily, on the first drag change, rather than
        // in onNodeDragStart: ReactFlow's first drag event already carries the
        // node moved by the first pointer step, so reading the position here —
        // before this change is applied to our own state — is what keeps the
        // node under the pointer instead of trailing it by one step.
        let origin = dragOrigins.current.get(change.id)
        if (!origin && change.dragging) {
          // Read the pre-drag position from our own state mirror, not from the
          // ReactFlow store: the drag machinery mutates its node lookup before
          // calling onNodesChange, so the store already holds the node moved by
          // the first pointer step. Anchoring on the stale store position would
          // leave the node trailing the pointer by that step for the whole
          // gesture, and persist the shortfall.
          const current = nodesById.current.get(change.id)
          if (current) {
            origin = { x: current.position.x, y: current.position.y }
            dragOrigins.current.set(change.id, origin)
          }
        }
        if (!origin) return change
        const worldDelta = unprojectDelta({
          x: change.position.x - origin.x,
          y: change.position.y - origin.y,
        })
        return {
          ...change,
          position: {
            x: origin.x + worldDelta.x,
            y: origin.y + worldDelta.y,
          },
        }
      })
      onNodesChange(projected)
      for (const change of projected) {
        if (change.type === 'position' && change.dragging === false) {
          dragOrigins.current.delete(change.id)
        }
      }
      for (const change of projected) {
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
    [commitNodePosition, onNodesChange, visualMode],
  )
  /**
   * 2.5D placement layer.
   *
   * `node.position` always stays in unprojected world coordinates — that is
   * what gets persisted and what the minimap, fit maths, and 2D mode read.
   * The projected screen position is applied as a CSS `translate`, which
   * composes with the `transform: translate(...)` ReactFlow writes for the
   * node's world position, so the rendered anchor lands exactly on
   * `projectPoint(worldPosition)` without ever mutating stored geometry.
   */
  const projectedNodes = useMemo<RuntimeNode[]>(() => {
    nodesById.current = new Map(nodes.map((node) => [node.id, node]))
    if (visualMode !== 'depth') return nodes
    const absolute = absoluteNodePositions(nodes)
    return nodes.map((node) => {
      const world = absolute.get(node.id) ?? node.position
      const anchor = projectPoint(world)
      const style = {
        ...node.style,
        translate: `${anchor.x - world.x}px ${anchor.y - world.y}px`,
      } as ProjectedNodeProperties
      if (node.type === 'project' || node.type === 'workspace') {
        const width = numericDimension(node.style?.width, 350)
        const height = numericDimension(node.style?.height, projectHeight(1))
        // The region element keeps existing as the territory's DOM stand-in for
        // state, aria, and the billboards parented to it, so its box has to
        // cover the projected envelope rather than the flat rectangle. The
        // extra margin leaves room for billboards standing on anchors near the
        // territory's far corners.
        const east = projectDelta({ x: width, y: 0 })
        const west = projectDelta({ x: 0, y: height })
        const boxLeft = west.x - TERRITORY_BOX_PAD
        const boxTop = -TERRITORY_BOX_PAD
        style['--territory-box-left'] = `${boxLeft}px`
        style['--territory-box-top'] = `${boxTop}px`
        // The upright territory label is pinned to the territory's projected
        // centroid so it reads as belonging to the parallelogram the user sees
        // and stays a valid allocation drop point. Anchoring it to the flat
        // box's top edge would put it outside the projected shape entirely.
        // Offsets are relative to the region box, not the node origin, because
        // that box is itself shifted to cover the projected envelope.
        const centre = projectDelta({ x: width / 2, y: height / 2 })
        style['--territory-label-x'] = `${centre.x - boxLeft}px`
        style['--territory-label-y'] = `${centre.y - boxTop}px`
        style['--territory-box-width'] =
          `${east.x - west.x + TERRITORY_BOX_PAD * 2 + WORKER_NODE_WIDTH}px`
        style['--territory-box-height'] =
          `${east.y + west.y + TERRITORY_BOX_PAD * 2 + WORKER_NODE_HEIGHT}px`
        // A territory's visible shape is a parallelogram drawn by ProjectedMap,
        // so the flat rectangle must stop being the mouse target. ReactFlow
        // writes `pointerEvents` inline on the node wrapper and inline styles
        // beat any stylesheet rule, which is why this is set here rather than
        // in App.css. Descendants that must stay live (the upright label, the
        // connection handles, the resize controls) opt back in with
        // `pointer-events: auto` under `[data-visual-mode="depth"]`.
        style.pointerEvents = 'none'
        // The territory layer sits above the billboards standing on it so the
        // upright label is never buried under a worker marker. It intercepts
        // nothing (see pointerEvents above); only the label opts back in.
        style.zIndex = 10
      } else {
        // Billboard nodes shrink to their baseline drawn size. The inner
        // layout keeps its original pixel dimensions through --billboard-* so
        // content does not reflow. At distant zoom levels the inner root and
        // its pointer target may grow beyond this wrapper by the separate,
        // capped zoom compensation. Dimensions come from the declared style
        // rather than the measured box, because the measured box is already
        // scaled and would compound every render.
        const width = numericDimension(node.style?.width, WORKER_NODE_WIDTH)
        const height = numericDimension(node.style?.height, WORKER_NODE_HEIGHT)
        style['--billboard-width'] = `${width}px`
        style['--billboard-height'] = `${height}px`
        style['--billboard-scale'] = String(BILLBOARD_SCALE)
        style.width = width * BILLBOARD_SCALE
        style.height = height * BILLBOARD_SCALE
      }
      return { ...node, style } as RuntimeNode
    })
  }, [nodes, visualMode])
  const canvasStyle = useMemo<RuntimeCanvasProperties>(
    () => ({
      '--billboard-zoom-compensation': String(
        Math.min(
          BILLBOARD_ZOOM_COMPENSATION_MAX,
          Math.max(
            1,
            BILLBOARD_ZOOM_COMPENSATION_START / viewportZoom,
          ),
        ),
      ),
    }),
    [viewportZoom],
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
      // Coordination billboards are drawn scaled in 2.5D, so their measured box
      // must not become the persisted geometry.
      const width =
        visualMode === 'depth'
          ? node.placement.geometry.width
          : (flowNode.measured?.width ??
            numericDimension(
              flowNode.style?.width,
              node.placement.geometry.width,
            ))
      const height =
        visualMode === 'depth'
          ? node.placement.geometry.height
          : (flowNode.measured?.height ??
            numericDimension(
              flowNode.style?.height,
              node.placement.geometry.height,
            ))
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
      const duration = window.matchMedia('(prefers-reduced-motion: reduce)')
        .matches
        ? 0
        : 420
      if (visualMode === 'depth') {
        // `fitView` frames ReactFlow's flat node boxes. Once territories are
        // projected parallelograms those boxes are no longer what the user
        // sees, so the camera has to be framed on the projected envelope of the
        // arranged world rectangles instead. The envelope is computed from the
        // placements just written rather than read back from the flow instance,
        // which may not have re-rendered them yet.
        const bounds = mergeBounds([
          projectBounds({
            x: yardNode.position.x,
            y: yardNode.position.y,
            width: yardWidth,
            height: yardHeight,
          }),
          ...allPlacements.map(({ placement }) =>
            projectBounds({
              x: placement.x,
              y: placement.y,
              width: placement.width,
              height: placement.height,
            }),
          ),
        ])
        if (bounds.width > 0 && bounds.height > 0) {
          void flowInstance.fitBounds(
            {
              x: bounds.x,
              // Headroom for structures rising above their ground.
              y: bounds.y - BUILDING_HEADROOM,
              width: bounds.width,
              height: bounds.height + BUILDING_HEADROOM,
            },
            { duration, padding: 0.14 },
          )
          return
        }
      }
      void flowInstance.fitView({
        duration,
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
    visualMode,
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
            color: 'var(--text-muted)',
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
            const communicationState = communicationPathState(
              route,
              projectStatusReports[projectId],
            )
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
        const communicationState: CommunicationPathState =
          latestStatus === 'failed' || latestStatus === 'ambiguous'
            ? 'failed'
            : automation.state === 'active' &&
                (latestStatus === 'pending' ||
                  latestStatus === 'submitted')
              ? 'active'
              : 'idle'
        return [
          {
            id: `automation-target:${automation.id}`,
            source: `automation:${automation.id}`,
            sourceHandle: 'automation-source',
            target,
            targetHandle: 'automation-target',
            type: 'smoothstep',
            animated: communicationState === 'active',
            selectable: false,
            deletable: false,
            reconnectable: false,
            focusable: false,
            zIndex: 2,
            className: `automation-target-edge automation-target-edge--${automation.state} automation-target-edge--${latestStatus}`,
            data: { communicationState },
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

  /**
   * The projected scene, derived from exactly the same node and edge model the
   * flat mode renders. There is no second data source and no parallel state:
   * territories, skylines, ground anchors, and routes are all read out of
   * `nodes`/`edges` and expressed in world coordinates, then projected once at
   * render time inside `ProjectedMap`.
   */
  const scene = useMemo<ProjectedScene>(() => {
    if (visualMode !== 'depth') return EMPTY_SCENE
    const absolute = absoluteNodePositions(nodes)
    const territories: ProjectedTerritory[] = []
    const anchors: ProjectedAnchor[] = []
    const centreByNodeId = new Map<string, WorldPoint>()

    for (const node of nodes) {
      const world = absolute.get(node.id) ?? node.position
      if (node.type === 'project') {
        const data = node.data as ProjectNodeData
        const { width, height } = nodeDimensions(
          node,
          Math.max(data.project.placement.geometry.width, data.minimumWidth),
          Math.max(data.project.placement.geometry.height, data.minimumHeight),
        )
        const rect = { x: world.x, y: world.y, width, height }
        centreByNodeId.set(node.id, {
          x: rect.x + rect.width / 2,
          y: rect.y + rect.height / 2,
        })
        territories.push({
          accent: data.accent,
          allocationTarget: data.isAllocationTarget,
          buildings: territoryBuildings(
            data.project.id,
            data.buildingCount,
            data.completedBuildingCount,
            rect,
          ),
          kind: 'project',
          label: data.project.name,
          nodeId: node.id,
          rect,
          runtime: data.workspace
            ? 'online'
            : data.runtimePending
              ? 'loading'
              : 'offline',
          selected: node.selected === true,
          status: data.workspace?.status ?? 'unknown',
          tokenTotal: data.projectTokenTotal,
        })
        continue
      }
      if (node.type === 'workspace') {
        const data = node.data as WorkspaceNodeData
        const accent =
          data.managedKind === 'yard_central'
            ? theme.tokens.statusDanger
            : data.managedKind === 'quarantined'
              ? theme.tokens.accentSecondary
            : data.managedKind === 'cleanup_pending'
              ? theme.tokens.statusWarning
              : theme.tokens.statusInfo
        const { width, height } = nodeDimensions(node, 350, projectHeight(1))
        const rect = { x: world.x, y: world.y, width, height }
        centreByNodeId.set(node.id, {
          x: rect.x + rect.width / 2,
          y: rect.y + rect.height / 2,
        })
        territories.push({
          accent,
          allocationTarget: false,
          buildings: [],
          kind: 'workspace',
          label: data.managedLabel ?? data.workspace.label,
          nodeId: node.id,
          rect,
          runtime: 'online',
          selected: node.selected === true,
          status: data.workspace.status,
        })
        continue
      }
      const kind = ANCHOR_KINDS[node.type]
      if (!kind) continue
      const measured = nodeDimensions(
        node,
        WORKER_NODE_WIDTH,
        WORKER_NODE_HEIGHT,
      )
      // Coordination nodes are scaled inside a correspondingly smaller
      // ReactFlow wrapper in depth mode. During a mode switch ReactFlow may
      // briefly retain the old flat-mode measurement, so derive their visible
      // footprint from the persisted dimensions instead of letting the ground
      // pad jump after a later measurement pass.
      const width =
        node.type === 'coordination-node'
          ? numericDimension(node.style?.width, measured.width) *
            BILLBOARD_SCALE
          : measured.width
      const height =
        node.type === 'coordination-node'
          ? numericDimension(node.style?.height, measured.height) *
            BILLBOARD_SCALE
          : measured.height
      // Billboards deliberately stay upright: only their anchor corner (the
      // node's own world position) is projected, then the rest of the box
      // extends as a flat, unskewed CSS offset from there — that's what
      // "billboard" means, as opposed to a building's fully projected faces.
      // Projecting a world-space offset point here, the way this used to
      // read, applies the axonometric skew to that offset too, which lands
      // nowhere near where the billboard actually renders. unprojectDelta is
      // the correct inverse: it finds the world-space delta that *produces*
      // the desired flat screen offset once projected, so this anchor point
      // reprojects back to the billboard's true visual bottom-center.
      const screenOffset = unprojectDelta({ x: width / 2, y: height })
      const centre = {
        x: world.x + screenOffset.x,
        y: world.y + screenOffset.y,
      }
      centreByNodeId.set(node.id, centre)
      anchors.push({
        accent:
          node.type === 'child-agent'
            ? (node.data as ChildAgentNodeData).accent
            : node.type === 'coordination-node'
              ? (node.data as CoordinationNodeData).node.kind ===
                'knowledge_store'
                ? theme.tokens.accentPrimary
                : theme.tokens.statusWarning
              : theme.tokens.statusDanger,
        kind,
        nodeId: node.id,
        point: centre,
        selected: node.selected === true,
        status: String(
          (node.data as { agent?: { status?: string } }).agent?.status ??
            'unknown',
        ),
      })
    }

    const routes: ProjectedRoute[] = edges.flatMap((edge): ProjectedRoute[] => {
      const from = centreByNodeId.get(edge.source)
      const to = centreByNodeId.get(edge.target)
      if (!from || !to) return []
      return [
        {
          from,
          id: edge.id,
          kind: routeKind(edge.id),
          state: routeState(edge),
          to,
        },
      ]
    })

    return { anchors, routes, territories }
  }, [edges, nodes, theme, visualMode])

  /**
   * Projected dragging for territories.
   *
   * Pointer capture keeps the gesture on the polygon the user grabbed. The
   * screen delta is divided by the live ReactFlow zoom and then inverse
   * projected, so pushing the pointer sideways slides the territory along both
   * ground axes and the stored world placement changes accordingly. Persistence
   * reuses `commitNodePosition`, the same call the flat drag path ends in.
   */
  const beginTerritoryDrag = useCallback(
    (nodeId: string, event: ReactPointerEvent<SVGElement>) => {
      if (visualMode !== 'depth' || event.button !== 0) return
      const flowInstance = instance.current
      const node = flowInstance?.getNode(nodeId)
      if (!flowInstance || !node || node.draggable === false) return
      event.stopPropagation()

      const surface = event.currentTarget
      const pointerId = event.pointerId
      const zoom = flowInstance.getViewport().zoom || 1
      const start = { x: event.clientX, y: event.clientY }
      const origin = { x: node.position.x, y: node.position.y }
      let latest = origin
      let moved = false
      const positionUpdates = createLatestFrameQueue<CanvasPoint>((position) => {
        setNodes((current) =>
          current.map((candidate) =>
            candidate.id === nodeId
              ? { ...candidate, position }
              : candidate,
          ),
        )
      })

      setNodes((current) =>
        current.map((candidate) => ({
          ...candidate,
          selected: candidate.id === nodeId,
        })),
      )
      onSelectionChange(selectionFromNodes([node]))

      const move = (moveEvent: PointerEvent) => {
        const worldDelta = unprojectDelta({
          x: (moveEvent.clientX - start.x) / zoom,
          y: (moveEvent.clientY - start.y) / zoom,
        })
        latest = {
          x: origin.x + worldDelta.x,
          y: origin.y + worldDelta.y,
        }
        if (
          Math.abs(moveEvent.clientX - start.x) > 2 ||
          Math.abs(moveEvent.clientY - start.y) > 2
        ) {
          moved = true
        }
        positionUpdates.schedule(latest)
      }

      const finish = () => {
        surface.removeEventListener('pointermove', move)
        surface.removeEventListener('pointerup', finish)
        surface.removeEventListener('pointercancel', finish)
        if (surface.hasPointerCapture?.(pointerId)) {
          surface.releasePointerCapture(pointerId)
        }
        positionUpdates.flush()
        if (!moved) return
        const settled = instance.current?.getNode(nodeId)
        if (settled) commitNodePosition(settled, latest)
      }

      surface.setPointerCapture?.(pointerId)
      surface.addEventListener('pointermove', move)
      surface.addEventListener('pointerup', finish)
      surface.addEventListener('pointercancel', finish)
    },
    [commitNodePosition, onSelectionChange, setNodes, visualMode],
  )

  /**
   * Frame the projected world once per switch into 2.5D.
   *
   * The flat mode's fixed default viewport assumes world coordinates land near
   * the screen origin. Projected coordinates do not: the world y axis runs left
   * across the screen, so a scene that was comfortably framed flat can sit
   * entirely off the left edge once projected. Framing the projected envelope
   * with `fitBounds` — not `fitView`, which frames ReactFlow's flat node boxes —
   * is what puts the map where the user is looking.
   */
  useEffect(() => {
    if (visualMode !== 'depth') {
      framedProjection.current = false
      return
    }
    if (framedProjection.current || nodes.length === 0) return
    const flowInstance = instance.current
    if (!flowInstance) return
    const absolute = absoluteNodePositions(nodes)
    const bounds = mergeBounds(
      nodes.map((node) => {
        const world = absolute.get(node.id) ?? node.position
        const { width, height } = nodeDimensions(
          node,
          WORKER_NODE_WIDTH,
          WORKER_NODE_HEIGHT,
        )
        return projectBounds({
          x: world.x,
          y: world.y,
          width,
          height,
        })
      }),
    )
    if (!(bounds.width > 0) || !(bounds.height > 0)) return
    framedProjection.current = true
    const compact = window.matchMedia('(max-width: 680px)').matches
    const base = compact ? { x: 18, y: 34, zoom: 0.72 } : { x: 44, y: 42, zoom: 0.88 }
    const frame = {
      x: bounds.x,
      // Headroom for the tallest structures, which rise above their ground.
      y: bounds.y - BUILDING_HEADROOM,
      width: bounds.width,
      height: bounds.height + BUILDING_HEADROOM,
    }
    // Frame the projected envelope, then refuse to zoom in past the mode's own
    // default. The projected world is wider than the flat one — world y runs
    // left across the screen — so on a narrow viewport it has to be zoomed out
    // to stay reachable, but on a roomy one it should be reframed, not
    // magnified.
    void flowInstance
      .fitBounds(frame, { duration: 0, padding: 0.06 })
      .then(() => {
        if (flowInstance.getViewport().zoom <= base.zoom) return
        void flowInstance.setViewport({
          x: base.x - frame.x * base.zoom,
          y: base.y - frame.y * base.zoom,
          zoom: base.zoom,
        })
      })
  }, [nodes, visualMode])

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
        theme,
        projectAccents,
        projectStatusReports,
        projects,
        runtimeLoading,
        runtimeTopology,
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
    theme,
    projectAccents,
    projects,
    projectStatusReports,
    runtimeLoading,
    runtimeTopology,
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
      className="runtime-canvas"
      colorMode={theme.colorScheme}
      data-visual-mode={visualMode}
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
      nodes={projectedNodes}
      nodesConnectable
      style={canvasStyle}
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
      onViewportChange={({ zoom }) => {
        setViewportZoom((current) =>
          Math.abs(current - zoom) < 0.001 ? current : zoom,
        )
      }}
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
          visualMode,
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
          ? projectAtPoint(
              instance.current?.getNodes() ?? [],
              point,
              visualMode,
            )?.id
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
      {visualMode === 'depth' ? (
        <ViewportPortal>
          <ProjectedMap
            onGroundContextMenu={(event) => showContextMenu(event)}
            onTerritoryPointerDown={beginTerritoryDrag}
            scene={scene}
          />
        </ViewportPortal>
      ) : null}
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
        maskColor={theme.tokens.mapMask}
        nodeColor={(node) => {
          if (node.type === 'automation') return theme.tokens.statusInfo
          if (node.type === 'yard-orchestrator') return theme.tokens.statusDanger
          if (node.type === 'coordination-node') {
            return (node.data as CoordinationNodeData).node.kind ===
              'knowledge_store'
              ? theme.tokens.accentPrimary
              : theme.tokens.statusWarning
          }
          if (node.type !== 'project') return theme.tokens.statusDanger
          return (node.data as ProjectNodeData).accent
        }}
        pannable
        zoomable
      />
      <Controls position="bottom-right" showInteractive={false} />
    </ReactFlow>
  )
}

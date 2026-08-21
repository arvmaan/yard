import {
  expect,
  test,
  type Locator,
  type Page,
  type WebSocketRoute,
} from '@playwright/test'
import type {
  Artifact,
  Assignment,
  Automation,
  AutomationRun,
  ChangeProjectOrchestratorInput,
  CompletionReceipt,
  ConfigureYardOrchestratorInput,
  CoordinationNode,
  CoordinationNodeRoute,
  CoordinationSnapshot,
  CreateProjectRelationshipInput,
  CreateCoordinationNodeInput,
  CreateAutomationInput,
  ConfirmedWorkerHandoff,
  ConfirmWorkerAssignmentInput,
  ConfirmWorkerHandoffInput,
  CreateProjectFromProfileInput,
  CreateWorkspaceProjectFromProfileInput,
  EndWorkerSessionInput,
  ObservedChildAgent,
  OrchestratorWorkflowProfile,
  ProjectRelationship,
  ProvisionYardOrchestratorInput,
  RecoverYardOrchestratorInput,
  ResetOrchestratorWorkflowProfileInput,
  ProvisionCoordinationNodeInput,
  RecordCompletionReceiptInput,
  RunAutomationInput,
  SendAssignmentPromptInput,
  SendCoordinationNodePromptInput,
  SendCoordinationNodeRouteInput,
  SendOrchestratorPromptInput,
  SendYardOrchestratorPromptInput,
  SendYardOrchestratorRouteInput,
  StatusReport,
  TokenSpendSettings,
  TerminalClientMessage,
  Worker,
  WorkerCandidate,
  WorkerRuntimeBinding,
  WorkspaceObservation,
  YardOrchestrator,
  YardOrchestratorRoute,
  UploadArtifactInput,
  RequestCoordinationSnapshotInput,
  UpdateCoordinationNodeInput,
  UpdateCoordinationNodePlacementInput,
  UpdateAutomationInput,
  UpdateAutomationPlacementInput,
  UpdateAutomationStateInput,
  UpdateOrchestratorWorkflowProfileInput,
  UpdateTokenSpendSettingsInput,
} from '../src/types'
import {
  assignmentTerminalWebSocketUrl,
  orchestratorTerminalWebSocketUrl,
} from '../src/api'
import {
  parseStatusReport,
  statusReportMatchesLatestRoute,
} from '../src/projectUpdates'

const sessions = {
  adapter: 'herdr',
  sessions: [
    { name: 'alpha', is_default: true, running: true },
    { name: 'beta', is_default: false, running: true },
  ],
}

const factoryWorkflowInstructions =
  '# Yard Orchestrator Workflow\n\nCreate one Yard/Herdr worker per lane and monitor every 10 minutes.\n'

function worker(index: number, workspaceId: string, status = 'idle') {
  return {
    runtime_id: `terminal-${index}`,
    terminal_id: `terminal-${index}`,
    workspace_id: workspaceId,
    tab_id: `${workspaceId}:tab-${index}`,
    pane_id: `${workspaceId}:pane-${index}`,
    name: index === 1 ? 'project-orchestrator' : `worker-${index}`,
    provider: index % 2 === 0 ? 'claude' : 'codex',
    display_provider: index % 2 === 0 ? 'Claude' : 'Codex',
    status,
    focused: index === 1,
    launch_pending: false,
    interactive_ready: true,
    state_change_sequence: String(index),
    cwd: `/tmp/sample/project-${workspaceId}`,
    foreground_cwd: `/tmp/sample/project-${workspaceId}`,
    tokens: {},
    provider_session: {
      source: 'synthetic',
      provider: index % 2 === 0 ? 'claude' : 'codex',
      kind: 'id',
      value: `synthetic-session-${index}`,
    },
    revision: String(index),
  }
}

function workspace(index: number): WorkspaceObservation {
  const runtimeId = `workspace-${index}`
  return {
    runtime_id: runtimeId,
    order: index,
    label: ['API migration', 'Release checks', 'Search indexing', 'Docs refresh'][
      index - 1
    ],
    focused: index === 1,
    active_tab_id: `${runtimeId}:tab-1`,
    pane_count: index === 1 ? 6 : 1,
    tab_count: index === 1 ? 6 : 1,
    status: index === 3 ? 'blocked' : 'idle',
    tokens: {},
    worktree: null,
  }
}

const workers = [
  ...Array.from({ length: 6 }, (_, index) =>
    worker(index + 1, 'workspace-1', index === 3 ? 'working' : 'idle'),
  ),
  worker(7, 'workspace-2'),
  worker(8, 'workspace-3', 'blocked'),
  worker(9, 'workspace-4'),
]

const inventory = {
  adapter: 'herdr',
  session: 'alpha',
  runtime_version: '0.8.0',
  protocol: 19,
  observed_at_unix_ms: 1_786_400_000_000,
  focus: {
    workspace_id: 'workspace-1',
    tab_id: 'workspace-1:tab-1',
    pane_id: 'workspace-1:pane-1',
  },
  workspaces: [workspace(1), workspace(2), workspace(3), workspace(4)],
  tabs: [],
  panes: [],
  workers,
  child_agents: [] as ObservedChildAgent[],
}

function profile(id = 'profile-1', name = 'Implementer') {
  return {
    id,
    name,
    runtime_adapter: 'herdr',
    provider: 'codex',
    model: 'gpt-5.4',
    default_role: 'implementer',
    instructions_ref: null,
    tools: [],
    skills: [],
    mcp_servers: [],
    sandbox_policy: 'runtime_default',
    worktree_policy: 'project_workspace',
    permission_policy: 'runtime_default',
    completion_contract: 'manual_receipt',
    version: '1',
    created_at_unix_ms: 1_786_400_000_000,
    updated_at_unix_ms: 1_786_400_000_000,
  }
}

function durableWorker(
  id: string,
  terminalId: string | null,
  workerProfile: ReturnType<typeof profile> | null = null,
  workspaceId = 'workspace-1',
  session = 'alpha',
  ownsTab = true,
  runtimeState: Partial<
    Pick<
      WorkerRuntimeBinding,
      'observation_state' | 'process_state' | 'status'
    >
  > = {},
): Worker {
  const now = 1_786_400_000_000
  const observedIndex = terminalId?.match(/^terminal-(\d+)$/)?.[1]
  const topologyId = observedIndex ?? terminalId
  const observedIndexNumber = observedIndex
    ? Number(observedIndex)
    : null
  return {
    id,
    profile_id: workerProfile?.id ?? null,
    profile_version: workerProfile?.version ?? null,
    desired_state: 'running',
    runtime: terminalId
      ? {
          adapter: 'herdr',
          session,
          workspace_id: workspaceId,
          terminal_id: terminalId,
          tab_id: `${workspaceId}:tab-${topologyId}`,
          pane_id: `${workspaceId}:pane-${topologyId}`,
          provider_session:
            observedIndexNumber === null
              ? null
              : {
                  source: 'synthetic',
                  provider:
                    observedIndexNumber % 2 === 0 ? 'claude' : 'codex',
                  kind: 'id',
                  value: `synthetic-session-${observedIndexNumber}`,
                },
          owns_tab: ownsTab,
          observation_state: 'observed',
          process_state: 'running',
          status: 'idle',
          state_change_sequence: '1',
          revision: '1',
          version: '1',
          last_observed_at_unix_ms: now,
          ...runtimeState,
        }
      : null,
    version: '1',
    created_at_unix_ms: now,
    updated_at_unix_ms: now,
  }
}

function project(
  id: string,
  name: string,
  session: string,
  workspaceId: string,
  terminalId: string,
  x: number,
) {
  return {
    id,
    name,
    runtime: {
      adapter: 'herdr',
      session,
      workspace_id: workspaceId,
    },
    orchestrator: durableWorker(
      `${id}-orchestrator`,
      terminalId,
      null,
      workspaceId,
      session,
      false,
      session === 'gamma'
        ? {
            observation_state: 'missing',
            process_state: 'unknown',
            status: 'unknown',
          }
        : {},
    ),
    placement: {
      geometry: {
        x,
        y: 70,
        width: 322,
        height: id === 'project-1' ? 364 : 240,
      },
      version: '1',
      updated_at_unix_ms: 1_786_400_000_000,
    },
    version: '1',
    created_at_unix_ms: 1_786_400_000_000,
    updated_at_unix_ms: 1_786_400_000_000,
  }
}

function initialProjects() {
  return [
    project(
      'project-1',
      'API migration',
      'alpha',
      'workspace-1',
      'terminal-1',
      80,
    ),
    project(
      'project-2',
      'Offline release',
      'gamma',
      'workspace-offline',
      'terminal-offline',
      470,
    ),
  ]
}

function initialWorkerCandidates(
  projects: ReturnType<typeof initialProjects>,
  profiles: ReturnType<typeof profile>[],
): WorkerCandidate[] {
  return [
    {
      worker: projects[0].orchestrator,
      availability: 'orchestrator',
      project_id: projects[0].id,
      reason: 'Project orchestrators cannot be reallocated.',
    },
    {
      worker: durableWorker(
        'worker-unassigned',
        'terminal-2',
        null,
        'workspace-1',
      ),
      availability: 'unassigned_live',
      reason: 'Live worker is not assigned to a project.',
    },
    {
      worker: durableWorker(
        'worker-resumable',
        'terminal-7',
        profiles[0],
        'workspace-2',
        'alpha',
        true,
        {
          process_state: 'exited',
          status: 'idle',
        },
      ),
      profile_name: profiles[0].name,
      default_role: profiles[0].default_role,
      availability: 'resumable',
      assignment_id: 'assignment-completed',
      reason: 'The previous assignment is complete.',
    },
    {
      worker: durableWorker('worker-assigned', 'terminal-3', profiles[0]),
      profile_name: profiles[0].name,
      default_role: profiles[0].default_role,
      availability: 'assigned',
      project_id: projects[0].id,
      assignment_id: 'assignment-active',
    },
    {
      worker: durableWorker(
        'worker-unavailable',
        'terminal-missing',
        profiles[0],
        'workspace-1',
        'alpha',
        true,
        {
          observation_state: 'missing',
          process_state: 'unknown',
          status: 'unknown',
        },
      ),
      profile_name: profiles[0].name,
      default_role: profiles[0].default_role,
      availability: 'unavailable',
      reason: 'Runtime binding is unavailable.',
    },
    {
      worker: durableWorker(
        'worker-ambiguous',
        'terminal-ambiguous',
        null,
        'workspace-1',
        'alpha',
        true,
        {
          observation_state: 'ambiguous',
          process_state: 'unknown',
          status: 'unknown',
        },
      ),
      availability: 'ambiguous',
      reason: 'Worker ownership could not be reconciled.',
    },
  ]
}

interface MockState {
  orchestratorWorkflowProfile: OrchestratorWorkflowProfile
  orchestratorWorkflowUpdates: UpdateOrchestratorWorkflowProfileInput[]
  orchestratorWorkflowResets: ResetOrchestratorWorkflowProfileInput[]
  tokenSpendSettings: TokenSpendSettings
  tokenSpendSettingsUpdates: UpdateTokenSpendSettingsInput[]
  automations: Automation[]
  automationRuns: Record<string, AutomationRun[]>
  automationCreateCommands: CreateAutomationInput[]
  automationUpdateCommands: UpdateAutomationInput[]
  automationPlacementCommands: UpdateAutomationPlacementInput[]
  automationStateCommands: UpdateAutomationStateInput[]
  automationRunCommands: RunAutomationInput[]
  coordinationNodes: CoordinationNode[]
  coordinationNodeCreateCommands: CreateCoordinationNodeInput[]
  coordinationNodeUpdateCommands: UpdateCoordinationNodeInput[]
  coordinationNodePlacementCommands: UpdateCoordinationNodePlacementInput[]
  coordinationNodeProvisionCommands: ProvisionCoordinationNodeInput[]
  coordinationNodePromptCommands: SendCoordinationNodePromptInput[]
  coordinationNodeRouteCommands: SendCoordinationNodeRouteInput[]
  coordinationNodeRoutes: CoordinationNodeRoute[]
  coordinationSnapshotCommands: RequestCoordinationSnapshotInput[]
  coordinationSnapshots: CoordinationSnapshot[]
  yardOrchestrator: YardOrchestrator
  yardOrchestratorConfigureCommands: ConfigureYardOrchestratorInput[]
  yardOrchestratorProvisionCommands: ProvisionYardOrchestratorInput[]
  yardOrchestratorRecoveryCommands: RecoverYardOrchestratorInput[]
  yardOrchestratorPromptCommands: SendYardOrchestratorPromptInput[]
  yardOrchestratorPromptDeliveries: SendYardOrchestratorPromptInput[]
  yardOrchestratorRouteCommands: SendYardOrchestratorRouteInput[]
  yardOrchestratorRoutes: YardOrchestratorRoute[]
  yardOrchestratorOutputRequests: string[]
  projectRelationshipCommands: CreateProjectRelationshipInput[]
  projectRelationships: ProjectRelationship[]
  projects: ReturnType<typeof initialProjects>
  projectRequests: number
  projectDetailRequests: number
  projectOrchestratorCommands: ChangeProjectOrchestratorInput[]
  assignmentRequests: number
  profiles: ReturnType<typeof profile>[]
  workerCandidates: WorkerCandidate[]
  workerRequests: number
  assignments: Assignment[]
  runtimeInventory: typeof inventory
  runtimeSessions: Array<{
    name: string
    is_default: boolean
    running: boolean
  }>
  inventoryFailure: boolean
  inventoryRequests: number
  inventoryRequestSessions: string[]
  inventoryResponsePlans: Map<
    string,
    Array<{
      observedAtUnixMs: number
      reconcileRuntimeTimestamps?: boolean
      wait?: Promise<void>
    }>
  >
  requestLog: string[]
  completionCommands: RecordCompletionReceiptInput[]
  completionRequestCommandIds: string[]
  artifacts: Map<string, { artifact: Artifact; content: string }>
  allocationCommands: ConfirmWorkerAssignmentInput[]
  allocationRequestCommandIds: string[]
  handoffCommands: ConfirmWorkerHandoffInput[]
  handoffRequestCommandIds: string[]
  endSessionCommands: EndWorkerSessionInput[]
  profileProjectCommands: CreateProjectFromProfileInput[]
  workspaceProjectCommands: CreateWorkspaceProjectFromProfileInput[]
  promptCommands: SendAssignmentPromptInput[]
  promptDeliveries: SendAssignmentPromptInput[]
  promptReplayCommandIds: string[]
  promptRequestCommandIds: string[]
  orchestratorPromptCommands: SendOrchestratorPromptInput[]
  orchestratorPromptDeliveries: SendOrchestratorPromptInput[]
  orchestratorPromptReplayCommandIds: string[]
  orchestratorPromptRequestCommandIds: string[]
  orchestratorTerminalOutputRequests: Array<{
    lines: string | null
    projectId: string
  }>
  terminalOutputRequests: Array<{
    assignmentId: string
    lines: string | null
    projectId: string
  }>
  terminalOutputFailure: boolean
  terminalConnectionUrls: string[]
  terminalMessages: TerminalClientMessage[]
  terminalSockets: WebSocketRoute[]
  ghosttyRequests: Array<{
    session: string
    terminalId: string
  }>
  placementUpdates: number
  automationPlacementUpdates: number
  conflictNextPlacement: boolean
}

function reconcileTransferRuntimeTimestamps(
  state: MockState,
  session: string,
  observedAtUnixMs: number,
) {
  state.runtimeInventory.observed_at_unix_ms = Math.max(
    state.runtimeInventory.observed_at_unix_ms,
    observedAtUnixMs,
  )
  state.projects = state.projects.map((project) =>
    project.runtime.session === session && project.orchestrator.runtime
      ? {
          ...project,
          orchestrator: {
            ...project.orchestrator,
            runtime: {
              ...project.orchestrator.runtime,
              last_observed_at_unix_ms: observedAtUnixMs,
            },
          },
        }
      : project,
  )
  state.workerCandidates = state.workerCandidates.map((candidate) =>
    candidate.worker.runtime?.session === session
      ? {
          ...candidate,
          worker: {
            ...candidate.worker,
            runtime: {
              ...candidate.worker.runtime,
              last_observed_at_unix_ms: observedAtUnixMs,
            },
          },
        }
      : candidate,
  )
}

function moveProjectTransferFixtureToSession(
  state: MockState,
  session: string,
) {
  const project = state.projects[0]
  project.runtime = { ...project.runtime, session }
  project.orchestrator = {
    ...project.orchestrator,
    runtime: {
      ...project.orchestrator.runtime!,
      session,
    },
  }
  const candidate = state.workerCandidates.find(
    ({ worker: candidateWorker }) =>
      candidateWorker.id === 'worker-unassigned',
  )
  if (!candidate?.worker.runtime) {
    throw new Error('Project transfer fixture is incomplete')
  }
  candidate.worker = {
    ...candidate.worker,
    runtime: {
      ...candidate.worker.runtime,
      session,
    },
  }
}

function deferred() {
  let resolve!: () => void
  const promise = new Promise<void>((complete) => {
    resolve = complete
  })
  return { promise, resolve }
}

function assignment(
  id: string,
  projectId: string,
  workerProfile: ReturnType<typeof profile>,
  objective: string,
  role: string,
  existingWorker?: Worker,
): Assignment {
  const now = Date.now()
  const assignedWorker = existingWorker
    ? {
        ...existingWorker,
        profile_id: workerProfile.id,
        profile_version: workerProfile.version,
        updated_at_unix_ms: now,
      }
    : durableWorker(
        `${id}-worker`,
        `${id}-terminal`,
        workerProfile,
        'workspace-1',
      )
  return {
    id,
    project_id: projectId,
    allocation_id: `${id}-allocation`,
    worker: assignedWorker,
    profile_id: workerProfile.id,
    profile_version: workerProfile.version,
    profile_name: workerProfile.name,
    objective,
    role,
    isolation_policy: 'project_workspace',
    lifecycle: 'active',
    attempt: {
      id: `${id}-attempt`,
      assignment_id: id,
      ordinal: 1,
      lifecycle: 'active',
      error: null,
      version: '2',
      created_at_unix_ms: now,
      updated_at_unix_ms: now,
    },
    completion_receipt: null as CompletionReceipt | null,
    version: '2',
    created_at_unix_ms: now,
    updated_at_unix_ms: now,
  }
}

async function mockApi(
  page: Page,
  options: {
    allocationFailsOnce?: boolean
    handoffFailsOnce?: boolean
    betaFails?: boolean
    completionDelayMs?: number
    completionFailsOnce?: boolean
    conflictNextPlacement?: boolean
    endSessionCleanupPending?: boolean
    promptDelayMs?: number
    promptDurableFailureOnce?:
      | 'runtime_intervention_ambiguous'
      | 'command_outcome_ambiguous'
      | 'command_previously_failed'
    promptLosesResponseOnce?: boolean
    projectOrchestratorStaleOnce?: boolean
    reconcileWorkerOnInventory?: boolean
    sessionWait?: Promise<void>
    orchestratorStatusReports?: Record<string, unknown>
    terminalOutputDelayMs?: number
    terminalOutputFails?: boolean
    terminalOutputText?: string | ((readCount: number) => string)
    terminalOutputTruncated?: boolean
    yardStatusReport?: unknown
    ghosttyFails?: boolean
  } = {},
) {
  const initialProjectState = initialProjects()
  const initialProfileState = [profile()]
  const state: MockState = {
    orchestratorWorkflowProfile: {
      id: 'yard:standard-orchestrator',
      name: 'Yard Standard Orchestrator',
      description: 'Provider-neutral orchestration.',
      version: '1',
      instructions_markdown: factoryWorkflowInstructions,
      monitor_interval_ms: '600000',
      commands: [
        {
          id: 'work.decompose',
          capability: 'orchestration.work.decompose',
        },
      ],
      adapter_context_files: [],
      source: 'factory',
      updated_by: 'yard:factory',
      created_at_unix_ms: 0,
    },
    orchestratorWorkflowUpdates: [],
    orchestratorWorkflowResets: [],
    tokenSpendSettings: {
      superintendent_auto_requests_project_summaries: false,
      project_orchestrators_auto_request_worker_summaries: false,
      scheduled_automatic_summaries: false,
      version: '1',
      updated_by: 'yard:migration',
      updated_at_unix_ms: 0,
    },
    tokenSpendSettingsUpdates: [],
    automations: [],
    automationRuns: {},
    automationCreateCommands: [],
    automationUpdateCommands: [],
    automationPlacementCommands: [],
    automationStateCommands: [],
    automationRunCommands: [],
    coordinationNodes: [],
    coordinationNodeCreateCommands: [],
    coordinationNodeUpdateCommands: [],
    coordinationNodePlacementCommands: [],
    coordinationNodeProvisionCommands: [],
    coordinationNodePromptCommands: [],
    coordinationNodeRouteCommands: [],
    coordinationNodeRoutes: [],
    coordinationSnapshotCommands: [],
    coordinationSnapshots: [],
    yardOrchestrator: {
      worker: null,
      version: '1',
      workflow_profile_version: '1',
      created_at_unix_ms: 0,
      updated_at_unix_ms: 0,
    },
    yardOrchestratorConfigureCommands: [],
    yardOrchestratorProvisionCommands: [],
    yardOrchestratorRecoveryCommands: [],
    yardOrchestratorPromptCommands: [],
    yardOrchestratorPromptDeliveries: [],
    yardOrchestratorRouteCommands: [],
    yardOrchestratorRoutes: [],
    yardOrchestratorOutputRequests: [],
    projectRelationshipCommands: [],
    projectRelationships: [],
    projects: initialProjectState,
    projectRequests: 0,
    projectDetailRequests: 0,
    projectOrchestratorCommands: [],
    assignmentRequests: 0,
    profiles: initialProfileState,
    workerCandidates: initialWorkerCandidates(
      initialProjectState,
      initialProfileState,
    ),
    workerRequests: 0,
    assignments: [],
    runtimeInventory: {
      ...inventory,
      focus: { ...inventory.focus },
      workspaces: inventory.workspaces.map((candidate) => ({ ...candidate })),
      tabs: [...inventory.tabs],
      panes: [...inventory.panes],
      workers: inventory.workers.map((candidate) => ({ ...candidate })),
      child_agents: inventory.child_agents.map((candidate) => ({
        ...candidate,
      })),
    },
    runtimeSessions: sessions.sessions.map((session) => ({ ...session })),
    inventoryFailure: false,
    inventoryRequests: 0,
    inventoryRequestSessions: [],
    inventoryResponsePlans: new Map(),
    requestLog: [],
    completionCommands: [],
    completionRequestCommandIds: [],
    artifacts: new Map(),
    allocationCommands: [],
    allocationRequestCommandIds: [],
    handoffCommands: [],
    handoffRequestCommandIds: [],
    endSessionCommands: [],
    profileProjectCommands: [],
    workspaceProjectCommands: [],
    promptCommands: [],
    promptDeliveries: [],
    promptReplayCommandIds: [],
    promptRequestCommandIds: [],
    orchestratorPromptCommands: [],
    orchestratorPromptDeliveries: [],
    orchestratorPromptReplayCommandIds: [],
    orchestratorPromptRequestCommandIds: [],
    orchestratorTerminalOutputRequests: [],
    terminalOutputRequests: [],
    terminalOutputFailure: options.terminalOutputFails ?? false,
    terminalConnectionUrls: [],
    terminalMessages: [],
    terminalSockets: [],
    ghosttyRequests: [],
    placementUpdates: 0,
    automationPlacementUpdates: 0,
    conflictNextPlacement: options.conflictNextPlacement ?? false,
  }
  let allocationFailsOnce = options.allocationFailsOnce ?? false
  let handoffFailsOnce = options.handoffFailsOnce ?? false
  let completionFailsOnce = options.completionFailsOnce ?? false
  let promptDurableFailureOnce = options.promptDurableFailureOnce
  let promptLosesResponseOnce = options.promptLosesResponseOnce ?? false
  let projectOrchestratorStaleOnce =
    options.projectOrchestratorStaleOnce ?? false
  const promptAcknowledgements = new Map<
    string,
    {
      acknowledgement: {
        command_id: string
        assignment_id: string
        attempt_id: string
        runtime_status: string
        submitted_at_unix_ms: number
      }
      command: SendAssignmentPromptInput
    }
  >()
  const orchestratorPromptAcknowledgements = new Map<
    string,
    {
      acknowledgement: {
        command_id: string
        project_id: string
        worker_id: string
        runtime_status: string
        submitted_at_unix_ms: number
      }
      command: SendOrchestratorPromptInput
    }
  >()
  const yardOrchestratorPromptAcknowledgements = new Map<
    string,
    {
      acknowledgement: {
        command_id: string
        worker_id: string
        runtime_status: string
        submitted_at_unix_ms: number
      }
      command: SendYardOrchestratorPromptInput
    }
  >()
  const handoffResults = new Map<
    string,
    {
      command: ConfirmWorkerHandoffInput
      result: ConfirmedWorkerHandoff
    }
  >()

  await page.route('**/api/v1/token-spend-settings', async (route) => {
    const request = route.request()
    if (request.method() === 'GET') {
      await route.fulfill({ json: state.tokenSpendSettings })
      return
    }
    if (request.method() === 'PUT') {
      const input =
        request.postDataJSON() as UpdateTokenSpendSettingsInput
      state.tokenSpendSettingsUpdates.push(input)
      if (input.expected_version !== state.tokenSpendSettings.version) {
        await route.fulfill({ status: 409 })
        return
      }
      state.tokenSpendSettings = {
        superintendent_auto_requests_project_summaries:
          input.superintendent_auto_requests_project_summaries,
        project_orchestrators_auto_request_worker_summaries:
          input.project_orchestrators_auto_request_worker_summaries,
        scheduled_automatic_summaries:
          input.scheduled_automatic_summaries,
        version: String(Number(state.tokenSpendSettings.version) + 1),
        updated_by: input.actor,
        updated_at_unix_ms: Date.now(),
      }
      await route.fulfill({ json: state.tokenSpendSettings })
      return
    }
    await route.fulfill({ status: 404 })
  })

  await page.route(
    '**/api/v1/orchestrator-workflow-profile',
    async (route) => {
      const request = route.request()
      if (request.method() === 'GET') {
        await route.fulfill({ json: state.orchestratorWorkflowProfile })
        return
      }
      if (request.method() === 'PUT') {
        const input =
          request.postDataJSON() as UpdateOrchestratorWorkflowProfileInput
        state.orchestratorWorkflowUpdates.push(input)
        if (
          input.expected_version !== state.orchestratorWorkflowProfile.version
        ) {
          await route.fulfill({ status: 409 })
          return
        }
        state.orchestratorWorkflowProfile = {
          ...state.orchestratorWorkflowProfile,
          version: String(
            Number(state.orchestratorWorkflowProfile.version) + 1,
          ),
          instructions_markdown: input.instructions_markdown,
          monitor_interval_ms: input.monitor_interval_ms,
          source: 'user',
          updated_by: input.actor,
          created_at_unix_ms: Date.now(),
        }
        await route.fulfill({ json: state.orchestratorWorkflowProfile })
        return
      }
      await route.fulfill({ status: 404 })
    },
  )

  await page.route(
    '**/api/v1/orchestrator-workflow-profile/reset',
    async (route) => {
      const input =
        route.request().postDataJSON() as ResetOrchestratorWorkflowProfileInput
      state.orchestratorWorkflowResets.push(input)
      if (
        input.expected_version !== state.orchestratorWorkflowProfile.version
      ) {
        await route.fulfill({ status: 409 })
        return
      }
      state.orchestratorWorkflowProfile = {
        ...state.orchestratorWorkflowProfile,
        version: String(
          Number(state.orchestratorWorkflowProfile.version) + 1,
        ),
        instructions_markdown: factoryWorkflowInstructions,
        monitor_interval_ms: '600000',
        source: 'reset',
        updated_by: input.actor,
        created_at_unix_ms: Date.now(),
      }
      await route.fulfill({ json: state.orchestratorWorkflowProfile })
    },
  )

  await page.routeWebSocket(
    (url) => url.pathname.endsWith('/terminal'),
    (socket) => {
      state.terminalConnectionUrls.push(socket.url())
      state.terminalSockets.push(socket)
      socket.onMessage((message) => {
        if (typeof message === 'string') {
          state.terminalMessages.push(
            JSON.parse(message) as TerminalClientMessage,
          )
        }
      })
    },
  )

  await page.route('**/api/v1/coordination-nodes**', async (route) => {
    const request = route.request()
    const url = new URL(request.url())
    const nodeMatch = url.pathname.match(
      /^\/api\/v1\/coordination-nodes\/([^/]+)$/,
    )
    const placementMatch = url.pathname.match(
      /^\/api\/v1\/coordination-nodes\/([^/]+)\/placement$/,
    )
    const provisionMatch = url.pathname.match(
      /^\/api\/v1\/coordination-nodes\/([^/]+)\/provision$/,
    )
    const promptMatch = url.pathname.match(
      /^\/api\/v1\/coordination-nodes\/([^/]+)\/prompts$/,
    )
    const outputMatch = url.pathname.match(
      /^\/api\/v1\/coordination-nodes\/([^/]+)\/terminal-output$/,
    )
    const routeMatch = url.pathname.match(
      /^\/api\/v1\/coordination-nodes\/([^/]+)\/routes$/,
    )
    const snapshotMatch = url.pathname.match(
      /^\/api\/v1\/coordination-nodes\/([^/]+)\/snapshots$/,
    )

    if (
      request.method() === 'GET' &&
      url.pathname === '/api/v1/coordination-nodes'
    ) {
      await route.fulfill({ json: { nodes: state.coordinationNodes } })
      return
    }
    if (
      request.method() === 'POST' &&
      url.pathname === '/api/v1/coordination-nodes'
    ) {
      const input = request.postDataJSON() as CreateCoordinationNodeInput
      state.coordinationNodeCreateCommands.push(input)
      const now = Date.now()
      const node: CoordinationNode = {
        id: crypto.randomUUID(),
        name: input.name,
        kind: input.kind,
        placement: {
          geometry: input.placement,
          version: '1',
          updated_at_unix_ms: now,
        },
        attached_project_ids: [...input.attached_project_ids],
        worker: null,
        cwd:
          input.kind === 'workstream'
            ? `/tmp/yard/coordination/${input.command_id}`
            : null,
        folder_path:
          input.kind === 'knowledge_store'
            ? `/tmp/yard/knowledge/${input.command_id}`
            : null,
        version: '1',
        created_by: input.actor,
        created_at_unix_ms: now,
        updated_at_unix_ms: now,
      }
      state.coordinationNodes.push(node)
      await route.fulfill({
        status: 201,
        json: {
          command_id: input.command_id,
          node,
          replayed: false,
        },
      })
      return
    }
    if (nodeMatch && request.method() === 'PUT') {
      const nodeId = decodeURIComponent(nodeMatch[1])
      const input = request.postDataJSON() as UpdateCoordinationNodeInput
      state.coordinationNodeUpdateCommands.push(input)
      const index = state.coordinationNodes.findIndex(
        (node) => node.id === nodeId,
      )
      const current = state.coordinationNodes[index]
      if (!current || current.version !== input.expected_version) {
        await route.fulfill({ status: 409 })
        return
      }
      const node: CoordinationNode = {
        ...current,
        name: input.name,
        attached_project_ids: [...input.attached_project_ids],
        version: String(Number(current.version) + 1),
        updated_at_unix_ms: Date.now(),
      }
      state.coordinationNodes[index] = node
      await route.fulfill({
        json: { command_id: input.command_id, node, replayed: false },
      })
      return
    }
    if (placementMatch && request.method() === 'PUT') {
      const nodeId = decodeURIComponent(placementMatch[1])
      const input =
        request.postDataJSON() as UpdateCoordinationNodePlacementInput
      state.coordinationNodePlacementCommands.push(input)
      const index = state.coordinationNodes.findIndex(
        (node) => node.id === nodeId,
      )
      const current = state.coordinationNodes[index]
      if (
        !current ||
        current.placement.version !== input.expected_version
      ) {
        await route.fulfill({ status: 409 })
        return
      }
      const node: CoordinationNode = {
        ...current,
        placement: {
          geometry: input.placement,
          version: String(Number(current.placement.version) + 1),
          updated_at_unix_ms: Date.now(),
        },
        updated_at_unix_ms: Date.now(),
      }
      state.coordinationNodes[index] = node
      await route.fulfill({
        json: { command_id: input.command_id, node, replayed: false },
      })
      return
    }
    if (provisionMatch && request.method() === 'POST') {
      const nodeId = decodeURIComponent(provisionMatch[1])
      const input = request.postDataJSON() as ProvisionCoordinationNodeInput
      state.coordinationNodeProvisionCommands.push(input)
      const index = state.coordinationNodes.findIndex(
        (node) => node.id === nodeId,
      )
      const current = state.coordinationNodes[index]
      const selectedProfile = state.profiles.find(
        (candidate) =>
          candidate.id === input.profile_id &&
          candidate.version === input.expected_profile_version,
      )
      if (
        !current ||
        !selectedProfile ||
        current.version !== input.expected_node_version
      ) {
        await route.fulfill({ status: 409 })
        return
      }
      const node: CoordinationNode = {
        ...current,
        worker: durableWorker(
          `coordination-worker-${state.coordinationNodes.length}`,
          `coordination-terminal-${state.coordinationNodes.length}`,
          selectedProfile,
          'coordination-workspace',
          'yard-coordination',
        ),
        version: String(Number(current.version) + 1),
        updated_at_unix_ms: Date.now(),
      }
      state.coordinationNodes[index] = node
      await route.fulfill({
        json: { command_id: input.command_id, node, replayed: false },
      })
      return
    }
    if (routeMatch && request.method() === 'GET') {
      const nodeId = decodeURIComponent(routeMatch[1])
      await route.fulfill({
        json: {
          routes: state.coordinationNodeRoutes.filter(
            (candidate) => candidate.node_id === nodeId,
          ),
        },
      })
      return
    }
    if (routeMatch && request.method() === 'POST') {
      const nodeId = decodeURIComponent(routeMatch[1])
      const input = request.postDataJSON() as SendCoordinationNodeRouteInput
      state.coordinationNodeRouteCommands.push(input)
      const now = Date.now()
      const routed: CoordinationNodeRoute = {
        ...input,
        node_id: nodeId,
        status: 'submitted',
        error_message: null,
        runtime_status: 'accepted',
        created_at_unix_ms: now,
        updated_at_unix_ms: now,
        submitted_at_unix_ms: now,
      }
      state.coordinationNodeRoutes.unshift(routed)
      await route.fulfill({ json: routed })
      return
    }
    if (snapshotMatch && request.method() === 'GET') {
      const nodeId = decodeURIComponent(snapshotMatch[1])
      await route.fulfill({
        json: {
          snapshots: state.coordinationSnapshots.filter(
            (snapshot) => snapshot.node_id === nodeId,
          ),
        },
      })
      return
    }
    if (snapshotMatch && request.method() === 'POST') {
      const nodeId = decodeURIComponent(snapshotMatch[1])
      const input =
        request.postDataJSON() as RequestCoordinationSnapshotInput
      state.coordinationSnapshotCommands.push(input)
      const node = state.coordinationNodes.find(
        (candidate) => candidate.id === nodeId,
      )
      if (!node || node.version !== input.expected_node_version) {
        await route.fulfill({ status: 409 })
        return
      }
      const now = Date.now()
      const snapshotId = crypto.randomUUID()
      const snapshot: CoordinationSnapshot = {
        id: snapshotId,
        node_id: nodeId,
        command_id: input.command_id,
        folder_path: `/tmp/yard/knowledge/${nodeId}/snapshots/${snapshotId}`,
        projects: node.attached_project_ids.map((projectId) => {
          const project = state.projects.find(
            (candidate) => candidate.id === projectId,
          )!
          return {
            project_id: projectId,
            project_version: project.version,
            orchestrator_worker_id: project.orchestrator.id,
            folder_path: `/tmp/yard/knowledge/${nodeId}/snapshots/${snapshotId}/projects/${projectId}`,
            collection_status: 'pending' as const,
            delivery_status: 'submitted' as const,
            delivery_error: null,
            runtime_status: 'accepted',
            submitted_at_unix_ms: now,
            collected_at_unix_ms: null,
          }
        }),
        progress: {
          completed: 0,
          total: node.attached_project_ids.length,
        },
        requested_by: input.actor,
        created_at_unix_ms: now,
        replayed: false,
      }
      state.coordinationSnapshots.unshift(snapshot)
      await route.fulfill({ status: 201, json: snapshot })
      return
    }
    if (outputMatch && request.method() === 'GET') {
      const nodeId = decodeURIComponent(outputMatch[1])
      const node = state.coordinationNodes.find(
        (candidate) => candidate.id === nodeId,
      )
      if (!node?.worker?.runtime) {
        await route.fulfill({ status: 409 })
        return
      }
      await route.fulfill({
        json: {
          node_id: nodeId,
          worker_id: node.worker.id,
          pane_id: node.worker.runtime.pane_id,
          source: 'herdr',
          format: 'text',
          text: 'Coordinating attached projects.',
          revision: '1',
          truncated: false,
          status_report: null,
        },
      })
      return
    }
    if (promptMatch && request.method() === 'POST') {
      const nodeId = decodeURIComponent(promptMatch[1])
      const input = request.postDataJSON() as SendCoordinationNodePromptInput
      state.coordinationNodePromptCommands.push(input)
      await route.fulfill({
        json: {
          command_id: input.command_id,
          node_id: nodeId,
          worker_id: input.worker_id,
          runtime_status: 'accepted',
          submitted_at_unix_ms: Date.now(),
        },
      })
      return
    }
    await route.fulfill({ status: 404 })
  })

  await page.route('**/api/v1/workers', async (route) => {
    if (route.request().method() === 'GET') {
      state.workerRequests += 1
      state.requestLog.push('workers')
      await route.fulfill({ json: { workers: state.workerCandidates } })
      return
    }
    await route.fulfill({ status: 404 })
  })

  await page.route('**/api/v1/workers/*/end-session', async (route) => {
    const request = route.request()
    const workerId = decodeURIComponent(
      new URL(request.url()).pathname.split('/')[4],
    )
    const input = request.postDataJSON() as EndWorkerSessionInput
    state.endSessionCommands.push(input)
    const index = state.workerCandidates.findIndex(
      (candidate) => candidate.worker.id === workerId,
    )
    const candidate = state.workerCandidates[index]
    if (!candidate) {
      await route.fulfill({ status: 404 })
      return
    }
    const endedWorker: Worker = {
      ...candidate.worker,
      desired_state: 'ended',
      runtime: null,
      version: String(Number(candidate.worker.version) + 1),
      updated_at_unix_ms: Date.now(),
    }
    state.workerCandidates[index] = {
      ...candidate,
      worker: endedWorker,
      availability: 'ended',
      project_id: null,
      assignment_id: null,
      reason: 'Session ended by explicit user action',
    }
    if (candidate.worker.runtime) {
      state.runtimeInventory.workers =
        state.runtimeInventory.workers.filter(
          (worker) =>
            worker.terminal_id !== candidate.worker.runtime?.terminal_id,
        )
    }
    await route.fulfill({
      json: {
        command_id: input.command_id,
        worker: endedWorker,
        cleanup_pending: options.endSessionCleanupPending ?? false,
        replayed: false,
      },
    })
  })

  await page.route('**/api/v1/project-relationships**', async (route) => {
    const request = route.request()
    const url = new URL(request.url())
    if (
      url.pathname === '/api/v1/project-relationships' &&
      request.method() === 'GET'
    ) {
      await route.fulfill({
        json: { relationships: state.projectRelationships },
      })
      return
    }
    if (
      url.pathname === '/api/v1/project-relationships' &&
      request.method() === 'POST'
    ) {
      const input =
        request.postDataJSON() as CreateProjectRelationshipInput
      state.projectRelationshipCommands.push(input)
      const relationship: ProjectRelationship = {
        id: input.relationship_id,
        source_project_id: input.source_project_id,
        target_project_id: input.target_project_id,
        kind: input.kind,
        version: '1',
        created_by: input.actor,
        created_at_unix_ms: Date.now(),
        updated_at_unix_ms: Date.now(),
      }
      state.projectRelationships.push(relationship)
      await route.fulfill({
        json: {
          command_id: input.command_id,
          relationship,
          replayed: false,
        },
      })
      return
    }
    const deleteMatch = url.pathname.match(
      /^\/api\/v1\/project-relationships\/([^/]+)\/delete$/,
    )
    if (deleteMatch && request.method() === 'POST') {
      const relationshipId = decodeURIComponent(deleteMatch[1])
      state.projectRelationships = state.projectRelationships.filter(
        (relationship) => relationship.id !== relationshipId,
      )
      await route.fulfill({
        json: {
          command_id: crypto.randomUUID(),
          relationship_id: relationshipId,
          deleted_at_unix_ms: Date.now(),
          replayed: false,
        },
      })
      return
    }
    await route.fulfill({ status: 404 })
  })

  await page.route('**/api/v1/yard/orchestrator**', async (route) => {
    const request = route.request()
    const url = new URL(request.url())
    if (
      url.pathname === '/api/v1/yard/orchestrator/recover' &&
      request.method() === 'POST'
    ) {
      const input =
        request.postDataJSON() as RecoverYardOrchestratorInput
      state.yardOrchestratorRecoveryCommands.push(input)
      if (
        input.expected_orchestrator_version !==
          state.yardOrchestrator.version ||
        !state.yardOrchestrator.worker
      ) {
        await route.fulfill({ status: 409 })
        return
      }
      const session = state.runtimeSessions.find(
        (candidate) => candidate.name === 'yard-orchestrator',
      )
      if (session) session.running = true
      else {
        state.runtimeSessions.push({
          name: 'yard-orchestrator',
          is_default: false,
          running: true,
        })
      }
      await route.fulfill({
        json: {
          command_id: input.command_id,
          orchestrator: state.yardOrchestrator,
        },
      })
      return
    }
    if (
      url.pathname === '/api/v1/yard/orchestrator' &&
      request.method() === 'GET'
    ) {
      await route.fulfill({ json: state.yardOrchestrator })
      return
    }
    if (
      url.pathname === '/api/v1/yard/orchestrator' &&
      request.method() === 'POST'
    ) {
      const input =
        request.postDataJSON() as ProvisionYardOrchestratorInput
      state.yardOrchestratorProvisionCommands.push(input)
      const selectedProfile = state.profiles.find(
        (candidate) =>
          candidate.id === input.profile_id &&
          candidate.version === input.expected_profile_version,
      )
      if (
        !selectedProfile ||
        state.yardOrchestrator.version !==
          input.expected_orchestrator_version
      ) {
        await route.fulfill({ status: 409 })
        return
      }
      const replacedWorkerId = state.yardOrchestrator.worker?.id ?? null
      const claimedWorker = durableWorker(
        'worker-yard-orchestrator',
        'terminal-yard-orchestrator',
        selectedProfile,
        'workspace-yard-orchestrator',
        'yard-orchestrator',
      )
      state.yardOrchestrator = {
        worker: claimedWorker,
        version: String(Number(state.yardOrchestrator.version) + 1),
        workflow_profile_version: state.orchestratorWorkflowProfile.version,
        created_at_unix_ms: state.yardOrchestrator.created_at_unix_ms,
        updated_at_unix_ms: Date.now(),
      }
      const yardSession = state.runtimeSessions.find(
        (candidate) => candidate.name === 'yard-orchestrator',
      )
      if (yardSession) yardSession.running = true
      else {
        state.runtimeSessions.push({
          name: 'yard-orchestrator',
          is_default: false,
          running: true,
        })
      }
      state.workerCandidates.push({
        worker: claimedWorker,
        profile_name: selectedProfile.name,
        default_role: selectedProfile.default_role,
        availability: 'yard_orchestrator',
        reason: 'Worker is the required Superintendent',
      })
      await route.fulfill({
        json: {
          command_id: input.command_id,
          orchestrator: state.yardOrchestrator,
          replaced_worker_id: replacedWorkerId,
          replayed: false,
        },
      })
      return
    }
    if (
      url.pathname === '/api/v1/yard/orchestrator' &&
      request.method() === 'PUT'
    ) {
      const input =
        request.postDataJSON() as ConfigureYardOrchestratorInput
      state.yardOrchestratorConfigureCommands.push(input)
      const candidateIndex = state.workerCandidates.findIndex(
        (candidate) => candidate.worker.id === input.worker_id,
      )
      const candidate = state.workerCandidates[candidateIndex]
      if (
        !candidate ||
        candidate.availability !== 'unassigned_live' ||
        candidate.worker.version !== input.expected_worker_version ||
        state.yardOrchestrator.version !==
          input.expected_orchestrator_version
      ) {
        await route.fulfill({ status: 409 })
        return
      }
      const replacedWorkerId = state.yardOrchestrator.worker?.id ?? null
      if (replacedWorkerId) {
        const replacedIndex = state.workerCandidates.findIndex(
          (current) => current.worker.id === replacedWorkerId,
        )
        if (replacedIndex >= 0) {
          state.workerCandidates[replacedIndex] = {
            ...state.workerCandidates[replacedIndex],
            availability: 'unassigned_live',
            reason: null,
          }
        }
      }
      const claimedWorker = {
        ...candidate.worker,
        version: String(Number(candidate.worker.version) + 1),
        updated_at_unix_ms: Date.now(),
      }
      state.workerCandidates[candidateIndex] = {
        ...candidate,
        worker: claimedWorker,
        availability: 'yard_orchestrator',
        reason: 'Worker is the required Superintendent',
      }
      state.yardOrchestrator = {
        worker: claimedWorker,
        version: String(Number(state.yardOrchestrator.version) + 1),
        workflow_profile_version: state.orchestratorWorkflowProfile.version,
        created_at_unix_ms: state.yardOrchestrator.created_at_unix_ms,
        updated_at_unix_ms: Date.now(),
      }
      await route.fulfill({
        json: {
          command_id: input.command_id,
          orchestrator: state.yardOrchestrator,
          replaced_worker_id: replacedWorkerId,
          replayed: false,
        },
      })
      return
    }
    if (
      url.pathname === '/api/v1/yard/orchestrator/routes' &&
      request.method() === 'GET'
    ) {
      await route.fulfill({ json: { routes: state.yardOrchestratorRoutes } })
      return
    }
    if (
      url.pathname === '/api/v1/yard/orchestrator/routes' &&
      request.method() === 'POST'
    ) {
      const input =
        request.postDataJSON() as SendYardOrchestratorRouteInput
      state.yardOrchestratorRouteCommands.push(input)
      const worker = state.yardOrchestrator.worker
      const project = state.projects.find(
        (candidate) => candidate.id === input.target_project_id,
      )
      if (
        !worker ||
        worker.id !== input.orchestrator_worker_id ||
        state.yardOrchestrator.version !==
          input.expected_orchestrator_version ||
        !project ||
        project.version !== input.expected_project_version ||
        project.orchestrator.id !==
          input.target_orchestrator_worker_id
      ) {
        await route.fulfill({ status: 409 })
        return
      }
      const now = Date.now()
      const routed: YardOrchestratorRoute = {
        ...input,
        status: 'submitted',
        error_message: null,
        runtime_status: 'accepted',
        created_at_unix_ms: now,
        updated_at_unix_ms: now,
        submitted_at_unix_ms: now,
      }
      state.yardOrchestratorRoutes = [
        routed,
        ...state.yardOrchestratorRoutes.filter(
          (candidate) =>
            candidate.command_id !== routed.command_id,
        ),
      ]
      await route.fulfill({ json: routed })
      return
    }
    if (
      url.pathname === '/api/v1/yard/orchestrator/terminal-output' &&
      request.method() === 'GET'
    ) {
      state.yardOrchestratorOutputRequests.push(
        url.searchParams.get('lines') ?? '',
      )
      const worker = state.yardOrchestrator.worker
      if (!worker?.runtime) {
        await route.fulfill({ status: 409 })
        return
      }
      await route.fulfill({
        json: {
          worker_id: worker.id,
          pane_id: worker.runtime.pane_id,
          source: 'herdr',
          format: 'text',
          text: 'Reviewing attention across the Yard portfolio.',
          revision: String(state.yardOrchestratorOutputRequests.length),
          truncated: false,
          status_report: options.yardStatusReport ?? null,
        },
      })
      return
    }
    if (
      url.pathname === '/api/v1/yard/orchestrator/prompts' &&
      request.method() === 'POST'
    ) {
      const input =
        request.postDataJSON() as SendYardOrchestratorPromptInput
      state.yardOrchestratorPromptCommands.push(input)
      const previous = yardOrchestratorPromptAcknowledgements.get(
        input.command_id,
      )
      if (previous) {
        await route.fulfill({ json: previous.acknowledgement })
        return
      }
      const worker = state.yardOrchestrator.worker
      if (
        !worker ||
        worker.id !== input.orchestrator_worker_id ||
        state.yardOrchestrator.version !==
          input.expected_orchestrator_version
      ) {
        await route.fulfill({ status: 409 })
        return
      }
      const acknowledgement = {
        command_id: input.command_id,
        worker_id: worker.id,
        runtime_status: 'accepted',
        submitted_at_unix_ms: Date.now(),
      }
      yardOrchestratorPromptAcknowledgements.set(input.command_id, {
        acknowledgement,
        command: input,
      })
      state.yardOrchestratorPromptDeliveries.push(input)
      await route.fulfill({ json: acknowledgement })
      return
    }
    await route.fulfill({ status: 404 })
  })

  await page.route('**/api/v1/worker-profiles**', async (route) => {
    const request = route.request()
    const url = new URL(request.url())
    const profileMatch = url.pathname.match(
      /^\/api\/v1\/worker-profiles\/([^/]+)$/,
    )
    if (request.method() === 'GET') {
      await route.fulfill({ json: { profiles: state.profiles } })
      return
    }
    if (
      request.method() === 'POST' &&
      url.pathname === '/api/v1/worker-profiles'
    ) {
      const input = request.postDataJSON()
      const created = {
        ...profile(`profile-${state.profiles.length + 1}`, input.name),
        ...input,
      }
      state.profiles.push(created)
      await route.fulfill({ status: 201, json: created })
      return
    }
    if (request.method() === 'PUT' && profileMatch) {
      const input = request.postDataJSON()
      const id = decodeURIComponent(profileMatch[1])
      const index = state.profiles.findIndex((candidate) => candidate.id === id)
      const updated = {
        ...state.profiles[index],
        ...input,
        id,
        version: String(Number(state.profiles[index].version) + 1),
      }
      delete updated.expected_version
      state.profiles[index] = updated
      await route.fulfill({ json: updated })
      return
    }
    await route.fulfill({ status: 404 })
  })

  await page.route('**/api/v1/projects**', async (route) => {
    const request = route.request()
    const url = new URL(request.url())
    const placementMatch = url.pathname.match(
      /^\/api\/v1\/projects\/([^/]+)\/placement$/,
    )
    const orchestratorTransferMatch = url.pathname.match(
      /^\/api\/v1\/projects\/([^/]+)\/orchestrator$/,
    )
    const assignmentMatch = url.pathname.match(
      /^\/api\/v1\/projects\/([^/]+)\/assignments$/,
    )
    const handoffMatch = url.pathname.match(
      /^\/api\/v1\/projects\/([^/]+)\/assignments\/([^/]+)\/handoffs$/,
    )
    const completionMatch = url.pathname.match(
      /^\/api\/v1\/projects\/([^/]+)\/assignments\/([^/]+)\/completion-receipts$/,
    )
    const artifactMatch = url.pathname.match(
      /^\/api\/v1\/projects\/([^/]+)\/assignments\/([^/]+)\/artifacts\/([^/]+)$/,
    )
    const artifactContentMatch = url.pathname.match(
      /^\/api\/v1\/projects\/([^/]+)\/assignments\/([^/]+)\/artifacts\/([^/]+)\/content$/,
    )
    const promptMatch = url.pathname.match(
      /^\/api\/v1\/projects\/([^/]+)\/assignments\/([^/]+)\/prompts$/,
    )
    const orchestratorPromptMatch = url.pathname.match(
      /^\/api\/v1\/projects\/([^/]+)\/orchestrator\/prompts$/,
    )
    const terminalOutputMatch = url.pathname.match(
      /^\/api\/v1\/projects\/([^/]+)\/assignments\/([^/]+)\/terminal-output$/,
    )
    const orchestratorTerminalOutputMatch = url.pathname.match(
      /^\/api\/v1\/projects\/([^/]+)\/orchestrator\/terminal-output$/,
    )
    const projectDetailMatch = url.pathname.match(
      /^\/api\/v1\/projects\/([^/]+)$/,
    )

    if (projectDetailMatch && request.method() === 'GET') {
      const projectId = decodeURIComponent(projectDetailMatch[1])
      const selectedProject = state.projects.find(
        (candidate) => candidate.id === projectId,
      )
      state.projectDetailRequests += 1
      state.requestLog.push(`project:${projectId}`)
      if (!selectedProject) {
        await route.fulfill({ status: 404 })
        return
      }
      await route.fulfill({ json: selectedProject })
      return
    }

    if (orchestratorTransferMatch && request.method() === 'PUT') {
      const projectId = decodeURIComponent(orchestratorTransferMatch[1])
      const input =
        request.postDataJSON() as ChangeProjectOrchestratorInput
      state.projectOrchestratorCommands.push(input)
      const projectIndex = state.projects.findIndex(
        (candidate) => candidate.id === projectId,
      )
      const workerIndex = state.workerCandidates.findIndex(
        (candidate) => candidate.worker.id === input.worker_id,
      )
      if (projectIndex < 0 || workerIndex < 0) {
        await route.fulfill({ status: 404 })
        return
      }

      let currentProject = state.projects[projectIndex]
      let selectedCandidate = state.workerCandidates[workerIndex]
      if (projectOrchestratorStaleOnce) {
        projectOrchestratorStaleOnce = false
        const now = Date.now()
        const bumpedOrchestrator = {
          ...currentProject.orchestrator,
          version: String(Number(currentProject.orchestrator.version) + 1),
          updated_at_unix_ms: now,
        }
        const bumpedWorker = {
          ...selectedCandidate.worker,
          version: String(Number(selectedCandidate.worker.version) + 1),
          updated_at_unix_ms: now,
        }
        currentProject = {
          ...currentProject,
          orchestrator: bumpedOrchestrator,
          version: String(Number(currentProject.version) + 1),
          updated_at_unix_ms: now,
        }
        selectedCandidate = {
          ...selectedCandidate,
          worker: bumpedWorker,
        }
        state.projects[projectIndex] = currentProject
        state.workerCandidates[workerIndex] = selectedCandidate
        const orchestratorCandidateIndex = state.workerCandidates.findIndex(
          (candidate) =>
            candidate.worker.id === bumpedOrchestrator.id,
        )
        if (orchestratorCandidateIndex >= 0) {
          state.workerCandidates[orchestratorCandidateIndex] = {
            ...state.workerCandidates[orchestratorCandidateIndex],
            worker: bumpedOrchestrator,
          }
        }
        await route.fulfill({
          status: 409,
          json: {
            error: {
              code: 'project_orchestrator_transfer_conflict',
              message:
                'Project ownership changed. Review the refreshed workers and retry.',
            },
          },
        })
        return
      }

      const workerRuntime = selectedCandidate.worker.runtime
      const orchestratorRuntime = currentProject.orchestrator.runtime
      const validIdentity =
        selectedCandidate.availability === 'unassigned_live' &&
        workerRuntime?.adapter === currentProject.runtime.adapter &&
        workerRuntime.session === currentProject.runtime.session &&
        workerRuntime.workspace_id === currentProject.runtime.workspace_id &&
        input.expected_worker_version === selectedCandidate.worker.version &&
        input.expected_project_version === currentProject.version &&
        input.expected_orchestrator_worker_id ===
          currentProject.orchestrator.id &&
        input.expected_orchestrator_worker_version ===
          currentProject.orchestrator.version &&
        JSON.stringify(input.expected_worker_runtime) ===
          JSON.stringify(workerRuntime) &&
        JSON.stringify(input.expected_orchestrator_runtime) ===
          JSON.stringify(orchestratorRuntime)
      if (!validIdentity) {
        await route.fulfill({
          status: 409,
          json: {
            error: {
              code: 'project_orchestrator_identity_changed',
              message:
                'Project or worker runtime identity changed. Refresh and retry.',
            },
          },
        })
        return
      }

      const now = Date.now()
      const replacedWorker = {
        ...currentProject.orchestrator,
        version: String(Number(currentProject.orchestrator.version) + 1),
        updated_at_unix_ms: now,
      }
      const nextOrchestrator = {
        ...selectedCandidate.worker,
        version: String(Number(selectedCandidate.worker.version) + 1),
        updated_at_unix_ms: now,
      }
      const updatedProject = {
        ...currentProject,
        orchestrator: nextOrchestrator,
        version: String(Number(currentProject.version) + 1),
        updated_at_unix_ms: now,
      }
      state.projects[projectIndex] = updatedProject
      state.workerCandidates[workerIndex] = {
        ...selectedCandidate,
        worker: nextOrchestrator,
        availability: 'orchestrator',
        project_id: currentProject.id,
        assignment_id: null,
        reason: 'Project orchestrators cannot be reallocated.',
      }
      const replacedCandidateIndex = state.workerCandidates.findIndex(
        (candidate) => candidate.worker.id === replacedWorker.id,
      )
      if (replacedCandidateIndex >= 0) {
        state.workerCandidates[replacedCandidateIndex] = {
          ...state.workerCandidates[replacedCandidateIndex],
          worker: replacedWorker,
          availability: 'unassigned_live',
          project_id: null,
          assignment_id: null,
          reason: 'Live worker is not assigned to a project.',
        }
      }
      await route.fulfill({
        json: {
          command_id: input.command_id,
          project: updatedProject,
          replaced_worker_id: replacedWorker.id,
          replayed: false,
        },
      })
      return
    }

    if (artifactContentMatch && request.method() === 'GET') {
      const artifactId = decodeURIComponent(artifactContentMatch[3])
      const stored = state.artifacts.get(artifactId)
      if (!stored) {
        await route.fulfill({ status: 404 })
        return
      }
      await route.fulfill({
        json: {
          artifact: stored.artifact,
          content: stored.content,
        },
      })
      return
    }

    if (artifactMatch && request.method() === 'PUT') {
      const projectId = decodeURIComponent(artifactMatch[1])
      const assignmentId = decodeURIComponent(artifactMatch[2])
      const artifactId = decodeURIComponent(artifactMatch[3])
      const input = request.postDataJSON() as UploadArtifactInput
      const current = state.assignments.find(
        (candidate) =>
          candidate.id === assignmentId &&
          candidate.project_id === projectId,
      )
      if (
        !current ||
        input.attempt_id !== current.attempt.id ||
        input.expected_assignment_version !== current.version ||
        input.expected_attempt_version !== current.attempt.version
      ) {
        await route.fulfill({ status: 409 })
        return
      }
      const existing = state.artifacts.get(artifactId)
      if (existing) {
        if (
          existing.content !== input.content ||
          existing.artifact.display_name !== input.display_name ||
          existing.artifact.kind !== input.kind
        ) {
          await route.fulfill({ status: 409 })
          return
        }
        await route.fulfill({ json: existing.artifact })
        return
      }
      const artifact: Artifact = {
        id: artifactId,
        project_id: projectId,
        assignment_id: assignmentId,
        attempt_id: current.attempt.id,
        worker_id: current.worker.id,
        kind: input.kind,
        media_type:
          input.kind === 'markdown' ? 'text/markdown' : 'text/html',
        display_name: input.display_name,
        byte_size: new TextEncoder().encode(input.content).byteLength,
        sha256: 'a'.repeat(64),
        source: 'upload',
        created_by: input.actor,
        created_at_unix_ms: Date.now(),
      }
      state.artifacts.set(artifactId, { artifact, content: input.content })
      await route.fulfill({ status: 201, json: artifact })
      return
    }

    if (handoffMatch && request.method() === 'POST') {
      const sourceProjectId = decodeURIComponent(handoffMatch[1])
      const sourceAssignmentId = decodeURIComponent(handoffMatch[2])
      const input = request.postDataJSON() as ConfirmWorkerHandoffInput
      state.handoffCommands.push(input)
      state.handoffRequestCommandIds.push(input.command_id)

      const previous = handoffResults.get(input.command_id)
      if (previous) {
        if (JSON.stringify(previous.command) !== JSON.stringify(input)) {
          await route.fulfill({
            status: 409,
            json: {
              error: {
                code: 'idempotency_conflict',
                message:
                  'Command ID is already associated with different input',
              },
            },
          })
          return
        }
        await route.fulfill({
          json: { ...previous.result, replayed: true },
        })
        return
      }
      if (handoffFailsOnce) {
        handoffFailsOnce = false
        await route.abort('failed')
        return
      }

      const sourceIndex = state.assignments.findIndex(
        (candidate) =>
          candidate.id === sourceAssignmentId &&
          candidate.project_id === sourceProjectId,
      )
      const source = state.assignments[sourceIndex]
      const sourceRuntime = source?.worker.runtime
      const sourceProjectIndex = state.projects.findIndex(
        (candidate) => candidate.id === sourceProjectId,
      )
      const targetProjectIndex = state.projects.findIndex(
        (candidate) => candidate.id === input.target_project_id,
      )
      if (
        !source ||
        !sourceRuntime ||
        source.worker.id !== input.worker_id ||
        source.lifecycle !== 'active' ||
        sourceProjectIndex < 0 ||
        targetProjectIndex < 0
      ) {
        await route.fulfill({ status: 409 })
        return
      }

      const now = Date.now()
      const targetProject = state.projects[targetProjectIndex]
      const ordinal = handoffResults.size + 1
      const movedWorker: Worker = {
        ...source.worker,
        runtime: {
          ...sourceRuntime,
          session: targetProject.runtime.session,
          workspace_id: targetProject.runtime.workspace_id,
          terminal_id: `terminal-handoff-${ordinal}`,
          tab_id: `${targetProject.runtime.workspace_id}:tab-handoff-${ordinal}`,
          pane_id: `${targetProject.runtime.workspace_id}:pane-handoff-${ordinal}`,
          provider_session: {
            source: 'synthetic',
            provider: 'codex',
            kind: 'id',
            value: `handoff-session-${ordinal}`,
          },
          state_change_sequence: '1',
          revision: '1',
          version: '1',
          last_observed_at_unix_ms: now,
        },
        version: String(Number(source.worker.version) + 2),
        updated_at_unix_ms: now,
      }
      const handedOffSource: Assignment = {
        ...source,
        worker: movedWorker,
        lifecycle: 'handed_off',
        attempt: {
          ...source.attempt,
          lifecycle: 'handed_off',
          version: String(Number(source.attempt.version) + 2),
          updated_at_unix_ms: now,
        },
        version: String(Number(source.version) + 2),
        updated_at_unix_ms: now,
      }
      const targetAssignment = assignment(
        `assignment-handoff-${ordinal}`,
        targetProject.id,
        state.profiles.find(
          (candidate) => candidate.id === source.profile_id,
        ) ?? state.profiles[0],
        input.objective,
        input.role,
        movedWorker,
      )

      state.assignments[sourceIndex] = handedOffSource
      state.assignments.push(targetAssignment)
      state.projects[sourceProjectIndex] = {
        ...state.projects[sourceProjectIndex],
        version: String(
          Number(state.projects[sourceProjectIndex].version) + 2,
        ),
        updated_at_unix_ms: now,
      }
      const replacedOrchestratorWorkerId =
        input.target_role === 'orchestrator'
          ? targetProject.orchestrator.id
          : null
      state.projects[targetProjectIndex] = {
        ...targetProject,
        ...(input.target_role === 'orchestrator'
          ? { orchestrator: movedWorker }
          : {}),
        version: String(Number(targetProject.version) + 2),
        updated_at_unix_ms: now,
      }

      const candidateIndex = state.workerCandidates.findIndex(
        (candidate) => candidate.worker.id === movedWorker.id,
      )
      const movedCandidate: WorkerCandidate = {
        worker: movedWorker,
        profile_name: targetAssignment.profile_name,
        default_role: targetAssignment.role,
        availability:
          input.target_role === 'orchestrator' ? 'orchestrator' : 'assigned',
        project_id: targetProject.id,
        assignment_id: targetAssignment.id,
        reason:
          input.target_role === 'orchestrator'
            ? 'Project orchestrators cannot be reallocated.'
            : undefined,
      }
      if (candidateIndex >= 0) {
        state.workerCandidates[candidateIndex] = movedCandidate
      } else {
        state.workerCandidates.push(movedCandidate)
      }

      const result: ConfirmedWorkerHandoff = {
        command_id: input.command_id,
        source_assignment: handedOffSource,
        allocation: {
          id: targetAssignment.allocation_id,
          project_id: targetProject.id,
          worker_id: movedWorker.id,
          mode: 'handoff',
          started_by_command_id: input.command_id,
          started_at_unix_ms: now,
        },
        assignment: targetAssignment,
        target_role: input.target_role,
        replaced_orchestrator_worker_id: replacedOrchestratorWorkerId,
        replayed: false,
      }
      handoffResults.set(input.command_id, { command: input, result })
      await route.fulfill({ status: 201, json: result })
      return
    }

    if (orchestratorTerminalOutputMatch && request.method() === 'GET') {
      const projectId = decodeURIComponent(orchestratorTerminalOutputMatch[1])
      const current = state.projects.find(
        (candidate) => candidate.id === projectId,
      )
      state.orchestratorTerminalOutputRequests.push({
        lines: url.searchParams.get('lines'),
        projectId,
      })
      if (!current?.orchestrator.runtime) {
        await route.fulfill({ status: 404 })
        return
      }
      if (options.terminalOutputDelayMs) {
        await new Promise((resolve) =>
          setTimeout(resolve, options.terminalOutputDelayMs),
        )
      }
      if (state.terminalOutputFailure) {
        await route.fulfill({
          status: 502,
          json: {
            error: {
              code: 'terminal_output_failed',
              message: 'Synthetic terminal output failure',
            },
          },
        })
        return
      }
      const readCount = state.orchestratorTerminalOutputRequests.filter(
        (request) => request.lines !== '80',
      ).length
      const configuredText = options.terminalOutputText
      const text =
        typeof configuredText === 'function'
          ? configuredText(readCount)
          : (configuredText ??
            `Recent orchestrator output read ${readCount}`)
      await route.fulfill({
        json: {
          project_id: current.id,
          worker_id: current.orchestrator.id,
          pane_id: current.orchestrator.runtime.pane_id,
          source: 'herdr',
          format: 'plain_text',
          text,
          revision: String(state.orchestratorTerminalOutputRequests.length),
          truncated: options.terminalOutputTruncated ?? false,
          status_report:
            options.orchestratorStatusReports?.[projectId] ?? null,
        },
      })
      return
    }

    if (terminalOutputMatch && request.method() === 'GET') {
      const projectId = decodeURIComponent(terminalOutputMatch[1])
      const assignmentId = decodeURIComponent(terminalOutputMatch[2])
      const current = state.assignments.find(
        (candidate) =>
          candidate.id === assignmentId &&
          candidate.project_id === projectId,
      )
      state.terminalOutputRequests.push({
        assignmentId,
        lines: url.searchParams.get('lines'),
        projectId,
      })
      if (!current) {
        await route.fulfill({ status: 404 })
        return
      }
      const runtime = current.worker.runtime
      if (!runtime) {
        await route.fulfill({ status: 409 })
        return
      }
      if (options.terminalOutputDelayMs) {
        await new Promise((resolve) =>
          setTimeout(resolve, options.terminalOutputDelayMs),
        )
      }
      if (state.terminalOutputFailure) {
        await route.fulfill({
          status: 502,
          json: {
            error: {
              code: 'terminal_output_failed',
              message: 'Synthetic terminal output failure',
            },
          },
        })
        return
      }
      const readCount = state.terminalOutputRequests.length
      const configuredText = options.terminalOutputText
      const text =
        typeof configuredText === 'function'
          ? configuredText(readCount)
          : (configuredText ?? `Recent terminal output read ${readCount}`)
      await route.fulfill({
        json: {
          assignment_id: current.id,
          attempt_id: current.attempt.id,
          pane_id: runtime.pane_id,
          source: 'herdr',
          format: 'plain_text',
          text,
          revision: String(readCount),
          truncated: options.terminalOutputTruncated ?? false,
        },
      })
      return
    }

    if (orchestratorPromptMatch && request.method() === 'POST') {
      const projectId = decodeURIComponent(orchestratorPromptMatch[1])
      const input = request.postDataJSON() as SendOrchestratorPromptInput
      const current = state.projects.find(
        (candidate) => candidate.id === projectId,
      )
      state.orchestratorPromptCommands.push(input)
      state.orchestratorPromptRequestCommandIds.push(input.command_id)
      if (!current) {
        await route.fulfill({ status: 404 })
        return
      }
      const previous = orchestratorPromptAcknowledgements.get(
        input.command_id,
      )
      if (previous) {
        if (JSON.stringify(previous.command) !== JSON.stringify(input)) {
          await route.fulfill({
            status: 409,
            json: {
              error: {
                code: 'idempotency_conflict',
                message:
                  'Command ID is already associated with different input',
              },
            },
          })
          return
        }
        state.orchestratorPromptReplayCommandIds.push(input.command_id)
        await route.fulfill({ json: previous.acknowledgement })
        return
      }
      if (
        input.expected_project_version !== current.version ||
        input.orchestrator_worker_id !== current.orchestrator.id
      ) {
        await route.fulfill({
          status: 409,
          json: {
            error: {
              code: 'orchestrator_changed',
              message: 'Project orchestrator changed concurrently',
            },
          },
        })
        return
      }
      if (promptDurableFailureOnce) {
        const code = promptDurableFailureOnce
        promptDurableFailureOnce = undefined
        await route.fulfill({
          status:
            code === 'runtime_intervention_ambiguous' ? 502 : 409,
          json: {
            error: {
              code,
              message: `Synthetic durable prompt failure: ${code}`,
            },
          },
        })
        return
      }

      const acknowledgement = {
        command_id: input.command_id,
        project_id: current.id,
        worker_id: current.orchestrator.id,
        runtime_status: 'submitted',
        submitted_at_unix_ms: Date.now(),
      }
      orchestratorPromptAcknowledgements.set(input.command_id, {
        acknowledgement,
        command: input,
      })
      state.orchestratorPromptDeliveries.push(input)
      if (promptLosesResponseOnce) {
        promptLosesResponseOnce = false
        await route.abort('failed')
        return
      }
      if (options.promptDelayMs) {
        await new Promise((resolve) =>
          setTimeout(resolve, options.promptDelayMs),
        )
      }
      await route.fulfill({ json: acknowledgement })
      return
    }

    if (promptMatch && request.method() === 'POST') {
      const projectId = decodeURIComponent(promptMatch[1])
      const assignmentId = decodeURIComponent(promptMatch[2])
      const input = request.postDataJSON() as SendAssignmentPromptInput
      const current = state.assignments.find(
        (candidate) =>
          candidate.id === assignmentId &&
          candidate.project_id === projectId,
      )
      state.promptCommands.push(input)
      state.promptRequestCommandIds.push(input.command_id)
      if (!current) {
        await route.fulfill({ status: 404 })
        return
      }
      if (
        input.attempt_id !== current.attempt.id ||
        input.expected_assignment_version !== current.version ||
        input.expected_attempt_version !== current.attempt.version
      ) {
        await route.fulfill({
          status: 409,
          json: {
            error: {
              code: 'assignment_version_conflict',
              message: 'Assignment or attempt changed concurrently',
            },
          },
        })
        return
      }
      const previous = promptAcknowledgements.get(input.command_id)
      if (previous) {
        if (JSON.stringify(previous.command) !== JSON.stringify(input)) {
          await route.fulfill({
            status: 409,
            json: {
              error: {
                code: 'idempotency_conflict',
                message:
                  'Command ID is already associated with different input',
              },
            },
          })
          return
        }
        state.promptReplayCommandIds.push(input.command_id)
        await route.fulfill({ json: previous.acknowledgement })
        return
      }
      if (promptDurableFailureOnce) {
        const code = promptDurableFailureOnce
        promptDurableFailureOnce = undefined
        await route.fulfill({
          status:
            code === 'runtime_intervention_ambiguous' ? 502 : 409,
          json: {
            error: {
              code,
              message: `Synthetic durable prompt failure: ${code}`,
            },
          },
        })
        return
      }

      const acknowledgement = {
        command_id: input.command_id,
        assignment_id: current.id,
        attempt_id: current.attempt.id,
        runtime_status: 'submitted',
        submitted_at_unix_ms: Date.now(),
      }
      promptAcknowledgements.set(input.command_id, {
        acknowledgement,
        command: input,
      })
      state.promptDeliveries.push(input)
      if (promptLosesResponseOnce) {
        promptLosesResponseOnce = false
        await route.abort('failed')
        return
      }
      if (options.promptDelayMs) {
        await new Promise((resolve) =>
          setTimeout(resolve, options.promptDelayMs),
        )
      }
      await route.fulfill({
        json: acknowledgement,
      })
      return
    }

    if (completionMatch && request.method() === 'POST') {
      const projectId = decodeURIComponent(completionMatch[1])
      const assignmentId = decodeURIComponent(completionMatch[2])
      const input = request.postDataJSON() as RecordCompletionReceiptInput
      state.completionRequestCommandIds.push(input.command_id)
      const index = state.assignments.findIndex(
        (candidate) =>
          candidate.id === assignmentId &&
          candidate.project_id === projectId,
      )
      const current = state.assignments[index]
      if (!current) {
        await route.fulfill({ status: 404 })
        return
      }
      if (
        input.attempt_id !== current.attempt.id ||
        input.expected_assignment_version !== current.version ||
        input.expected_attempt_version !== current.attempt.version
      ) {
        await route.fulfill({
          status: 409,
          json: {
            error: {
              code: 'assignment_version_conflict',
              message: 'Assignment or attempt changed concurrently',
            },
          },
        })
        return
      }
      if (completionFailsOnce) {
        completionFailsOnce = false
        await route.abort('failed')
        return
      }

      const now = Date.now()
      const receipt: CompletionReceipt = {
        id: `${assignmentId}-receipt`,
        assignment_id: assignmentId,
        attempt_id: current.attempt.id,
        outcome: 'completed',
        summary: input.summary,
        artifact_refs: input.artifact_refs,
        artifacts: input.artifact_ids.map((id) => {
          const stored = state.artifacts.get(id)
          if (!stored) throw new Error(`Missing mocked artifact ${id}`)
          return stored.artifact
        }),
        evidence_refs: input.evidence_refs,
        unresolved_blockers: input.unresolved_blockers,
        actor: input.actor,
        created_at_unix_ms: now,
      }
      const completed: Assignment = {
        ...current,
        lifecycle: 'completed',
        attempt: {
          ...current.attempt,
          lifecycle: 'completed',
          version: String(Number(current.attempt.version) + 1),
          updated_at_unix_ms: now,
        },
        completion_receipt: receipt,
        version: String(Number(current.version) + 1),
        updated_at_unix_ms: now,
      }
      state.completionCommands.push(input)
      state.assignments[index] = completed
      const candidateIndex = state.workerCandidates.findIndex(
        (candidate) => candidate.worker.id === completed.worker.id,
      )
      const resumableCandidate: WorkerCandidate = {
        worker: completed.worker,
        profile_name: completed.profile_name,
        default_role: completed.role,
        availability: 'resumable',
        assignment_id: completed.id,
        reason: 'The previous assignment is complete.',
      }
      if (candidateIndex >= 0) {
        state.workerCandidates[candidateIndex] = resumableCandidate
      } else {
        state.workerCandidates.push(resumableCandidate)
      }
      if (options.completionDelayMs) {
        await new Promise((resolve) =>
          setTimeout(resolve, options.completionDelayMs),
        )
      }
      await route.fulfill({
        status: 201,
        json: {
          command_id: input.command_id,
          receipt,
          assignment: completed,
          replayed: false,
        },
      })
      return
    }

    if (assignmentMatch && request.method() === 'GET') {
      state.assignmentRequests += 1
      const projectId = decodeURIComponent(assignmentMatch[1])
      await route.fulfill({
        json: {
          assignments: state.assignments.filter(
            (candidate) => candidate.project_id === projectId,
          ),
        },
      })
      return
    }

    if (assignmentMatch && request.method() === 'POST') {
      const projectId = decodeURIComponent(assignmentMatch[1])
      const input =
        request.postDataJSON() as ConfirmWorkerAssignmentInput
      state.allocationCommands.push(input)
      state.allocationRequestCommandIds.push(input.command_id)
      if (allocationFailsOnce) {
        allocationFailsOnce = false
        await route.abort('failed')
        return
      }

      const candidateIndex =
        'worker_id' in input
          ? state.workerCandidates.findIndex(
              (candidate) => candidate.worker.id === input.worker_id,
            )
          : -1
      const existingCandidate =
        candidateIndex >= 0
          ? state.workerCandidates[candidateIndex]
          : undefined
      const profileId =
        input.profile_id ?? existingCandidate?.worker.profile_id
      const workerProfile =
        state.profiles.find((candidate) => candidate.id === profileId) ??
        ('worker_id' in input &&
        existingCandidate &&
        !existingCandidate.worker.profile_id
          ? {
              ...state.profiles[0],
              id: 'yard:managed-blank-profile',
              name: 'Blank profile',
              default_role: 'worker',
            }
          : undefined)
      if (
        !workerProfile ||
        ('worker_id' in input && !existingCandidate)
      ) {
        await route.fulfill({ status: 404 })
        return
      }
      const created = assignment(
        `assignment-${state.assignments.length + 1}`,
        projectId,
        workerProfile,
        input.objective,
        input.role,
        existingCandidate?.worker,
      )
      state.assignments.push(created)
      const assignedCandidate: WorkerCandidate = {
        worker: created.worker,
        profile_name: workerProfile.name,
        default_role: input.role,
        availability: 'assigned',
        project_id: projectId,
        assignment_id: created.id,
      }
      if (candidateIndex >= 0) {
        state.workerCandidates[candidateIndex] = assignedCandidate
      } else {
        state.workerCandidates.push(assignedCandidate)
      }
      const projectIndex = state.projects.findIndex(
        (candidate) => candidate.id === projectId,
      )
      state.projects[projectIndex] = {
        ...state.projects[projectIndex],
        version: String(Number(state.projects[projectIndex].version) + 1),
      }
      await route.fulfill({
        status: 201,
        json: {
          command_id: input.command_id,
          allocation: {
            id: created.allocation_id,
            project_id: projectId,
            worker_id: created.worker.id,
            mode:
              'worker_id' in input
                ? existingCandidate?.availability === 'resumable'
                  ? 'resume'
                  : 'assign_existing'
                : 'create_new',
            started_by_command_id: input.command_id,
            started_at_unix_ms: created.created_at_unix_ms,
          },
          assignment: created,
          replayed: false,
        },
      })
      return
    }

    if (
      request.method() === 'POST' &&
      url.pathname === '/api/v1/projects/from-profile/workspace'
    ) {
      const input =
        request.postDataJSON() as CreateWorkspaceProjectFromProfileInput
      const prior = state.workspaceProjectCommands.find(
        (command) => command.command_id === input.command_id,
      )
      const existing = prior
        ? state.projects.find(
            (candidate) =>
              candidate.runtime.session === input.runtime_session &&
              candidate.name === input.name,
          )
        : undefined
      if (prior && existing) {
        await route.fulfill({
          json: {
            command_id: input.command_id,
            project: existing,
            replayed: true,
          },
        })
        return
      }

      const workerProfile = state.profiles.find(
        (candidate) => candidate.id === input.profile_id,
      )
      if (!workerProfile) {
        await route.fulfill({ status: 404 })
        return
      }
      state.workspaceProjectCommands.push(input)
      const ordinal = state.workspaceProjectCommands.length
      const workspaceId = `workspace-created-${ordinal}`
      const terminalId = `terminal-workspace-orchestrator-${ordinal}`
      const now = Date.now()
      const created = {
        ...project(
          `project-${state.projects.length + 1}`,
          input.name,
          input.runtime_session,
          workspaceId,
          terminalId,
          input.placement.x,
        ),
        orchestrator: durableWorker(
          `project-${state.projects.length + 1}-orchestrator`,
          terminalId,
          workerProfile,
          workspaceId,
          input.runtime_session,
        ),
        placement: {
          geometry: input.placement,
          version: '1',
          updated_at_unix_ms: now,
        },
        created_at_unix_ms: now,
        updated_at_unix_ms: now,
      }
      state.runtimeInventory.workspaces.push({
        ...workspace(state.runtimeInventory.workspaces.length + 1),
        runtime_id: workspaceId,
        label: input.workspace_label,
        active_tab_id: `${workspaceId}:tab-${terminalId}`,
        worktree: {
          repository_key: input.name.toLowerCase().replaceAll(' ', '-'),
          repository_name: input.name,
          repository_root: input.cwd,
          checkout_path: input.cwd,
          is_linked: false,
        },
      })
      state.runtimeInventory.workers.push({
        ...worker(10 + ordinal, workspaceId),
        runtime_id: terminalId,
        terminal_id: terminalId,
        tab_id: `${workspaceId}:tab-${terminalId}`,
        pane_id: `${workspaceId}:pane-${terminalId}`,
        name: 'workspace-orchestrator',
      })
      state.projects.push(created)
      state.workerCandidates.push({
        worker: created.orchestrator,
        profile_name: workerProfile.name,
        default_role: 'orchestrator',
        availability: 'orchestrator',
        project_id: created.id,
        reason: 'Project orchestrators cannot be reallocated.',
      })
      await route.fulfill({
        status: 201,
        headers: { Location: `/api/v1/projects/${created.id}` },
        json: {
          command_id: input.command_id,
          project: created,
          replayed: false,
        },
      })
      return
    }

    if (
      request.method() === 'POST' &&
      url.pathname === '/api/v1/projects/from-profile'
    ) {
      const input =
        request.postDataJSON() as CreateProjectFromProfileInput
      const prior = state.profileProjectCommands.find(
        (command) => command.command_id === input.command_id,
      )
      const existing = state.projects.find(
        (candidate) =>
          candidate.runtime.session === input.runtime.session &&
          candidate.runtime.workspace_id === input.runtime.workspace_id,
      )
      if (prior && existing) {
        await route.fulfill({
          json: {
            command_id: input.command_id,
            project: existing,
            replayed: true,
          },
        })
        return
      }

      const workerProfile = state.profiles.find(
        (candidate) => candidate.id === input.profile_id,
      )
      if (!workerProfile) {
        await route.fulfill({ status: 404 })
        return
      }
      state.profileProjectCommands.push(input)
      const terminalId = `terminal-profile-orchestrator-${state.projects.length + 1}`
      const now = Date.now()
      const created = {
        ...project(
          `project-${state.projects.length + 1}`,
          input.name,
          input.runtime.session,
          input.runtime.workspace_id,
          terminalId,
          input.placement.x,
        ),
        orchestrator: durableWorker(
          `project-${state.projects.length + 1}-orchestrator`,
          terminalId,
          workerProfile,
          input.runtime.workspace_id,
          input.runtime.session,
        ),
        placement: {
          geometry: input.placement,
          version: '1',
          updated_at_unix_ms: now,
        },
        created_at_unix_ms: now,
        updated_at_unix_ms: now,
      }
      const observed = {
        ...worker(10, input.runtime.workspace_id),
        runtime_id: terminalId,
        terminal_id: terminalId,
        tab_id: `${input.runtime.workspace_id}:tab-${terminalId}`,
        pane_id: `${input.runtime.workspace_id}:pane-${terminalId}`,
        name: 'profile-orchestrator',
      }
      state.runtimeInventory.workers.push(observed)
      state.projects.push(created)
      state.workerCandidates.push({
        worker: created.orchestrator,
        profile_name: workerProfile.name,
        default_role: 'orchestrator',
        availability: 'orchestrator',
        project_id: created.id,
        reason: 'Project orchestrators cannot be reallocated.',
      })
      await route.fulfill({
        status: 201,
        headers: { Location: `/api/v1/projects/${created.id}` },
        json: {
          command_id: input.command_id,
          project: created,
          replayed: false,
        },
      })
      return
    }

    if (request.method() === 'GET' && url.pathname === '/api/v1/projects') {
      state.projectRequests += 1
      await route.fulfill({ json: { projects: state.projects } })
      return
    }

    if (request.method() === 'POST' && url.pathname === '/api/v1/projects') {
      const input = request.postDataJSON()
      const observed = state.runtimeInventory.workers.find(
        (candidate) =>
          candidate.runtime_id === input.orchestrator_observed_worker_id,
      )
      if (!observed) {
        await route.fulfill({ status: 404 })
        return
      }
      const created = {
        ...project(
          `project-${state.projects.length + 1}`,
          input.name,
          input.runtime.session,
          input.runtime.workspace_id,
          observed.terminal_id,
          input.placement.x,
        ),
        placement: {
          geometry: input.placement,
          version: '1',
          updated_at_unix_ms: Date.now(),
        },
      }
      state.projects.push(created)
      await route.fulfill({
        status: 201,
        headers: { Location: `/api/v1/projects/${created.id}` },
        json: created,
      })
      return
    }

    if (request.method() === 'PUT' && placementMatch) {
      state.placementUpdates += 1
      if (state.conflictNextPlacement) {
        state.conflictNextPlacement = false
        await route.fulfill({
          status: 409,
          json: {
            error: {
              code: 'project_version_conflict',
              message:
                'Project placement changed concurrently; current version is 2',
            },
          },
        })
        return
      }
      const id = decodeURIComponent(placementMatch[1])
      const input = request.postDataJSON()
      const index = state.projects.findIndex((candidate) => candidate.id === id)
      const current = state.projects[index]
      const updated = {
        ...current,
        placement: {
          geometry: input.placement,
          version: String(Number(current.placement.version) + 1),
          updated_at_unix_ms: Date.now(),
        },
      }
      state.projects[index] = updated
      await route.fulfill({ json: updated })
      return
    }

    await route.fulfill({ status: 404 })
  })

  await page.route('**/api/v1/automations**', async (route) => {
    const request = route.request()
    const url = new URL(request.url())
    const detailMatch = url.pathname.match(
      /^\/api\/v1\/automations\/([^/]+)$/,
    )
    const placementMatch = url.pathname.match(
      /^\/api\/v1\/automations\/([^/]+)\/placement$/,
    )
    const stateMatch = url.pathname.match(
      /^\/api\/v1\/automations\/([^/]+)\/state$/,
    )
    const runsMatch = url.pathname.match(
      /^\/api\/v1\/automations\/([^/]+)\/runs$/,
    )

    if (
      request.method() === 'GET' &&
      url.pathname === '/api/v1/automations'
    ) {
      await route.fulfill({ json: { automations: state.automations } })
      return
    }
    if (
      request.method() === 'POST' &&
      url.pathname === '/api/v1/automations'
    ) {
      const input = request.postDataJSON() as CreateAutomationInput
      state.automationCreateCommands.push(input)
      const now = Date.now()
      const automation: Automation = {
        id: `automation-${state.automations.length + 1}`,
        name: input.name,
        scope: input.scope,
        placement: {
          geometry: input.placement,
          version: '1',
          updated_at_unix_ms: now,
        },
        schedule: input.schedule,
        selected_project_ids: [...input.selected_project_ids],
        prompt_template: input.prompt_template,
        state: 'active',
        next_run_at_unix_ms: now + 86_400_000,
        latest_run: null,
        version: '1',
        created_by: input.actor,
        created_at_unix_ms: now,
        updated_at_unix_ms: now,
      }
      state.automations.push(automation)
      state.automationRuns[automation.id] = []
      await route.fulfill({
        status: 201,
        json: {
          automation,
          command_id: input.command_id,
          replayed: false,
        },
      })
      return
    }
    if (detailMatch && request.method() === 'GET') {
      const automationId = decodeURIComponent(detailMatch[1])
      const automation = state.automations.find(
        (candidate) => candidate.id === automationId,
      )
      await route.fulfill(
        automation ? { json: automation } : { status: 404 },
      )
      return
    }
    if (detailMatch && request.method() === 'PUT') {
      const automationId = decodeURIComponent(detailMatch[1])
      const input = request.postDataJSON() as UpdateAutomationInput
      state.automationUpdateCommands.push(input)
      const index = state.automations.findIndex(
        (candidate) => candidate.id === automationId,
      )
      const current = state.automations[index]
      if (!current || current.version !== input.expected_version) {
        await route.fulfill({ status: 409 })
        return
      }
      const automation: Automation = {
        ...current,
        name: input.name,
        scope: input.scope,
        schedule: input.schedule,
        selected_project_ids: [...input.selected_project_ids],
        prompt_template: input.prompt_template,
        version: String(Number(current.version) + 1),
        updated_at_unix_ms: Date.now(),
      }
      state.automations[index] = automation
      await route.fulfill({
        json: {
          automation,
          command_id: input.command_id,
          replayed: false,
        },
      })
      return
    }
    if (placementMatch && request.method() === 'PUT') {
      const automationId = decodeURIComponent(placementMatch[1])
      const input =
        request.postDataJSON() as UpdateAutomationPlacementInput
      state.automationPlacementCommands.push(input)
      const index = state.automations.findIndex(
        (candidate) => candidate.id === automationId,
      )
      const current = state.automations[index]
      if (
        !current ||
        current.placement.version !== input.expected_version
      ) {
        await route.fulfill({ status: 409 })
        return
      }
      state.automationPlacementUpdates += 1
      const automation: Automation = {
        ...current,
        placement: {
          geometry: input.placement,
          version: String(Number(current.placement.version) + 1),
          updated_at_unix_ms: Date.now(),
        },
        updated_at_unix_ms: Date.now(),
      }
      state.automations[index] = automation
      await route.fulfill({ json: { automation } })
      return
    }
    if (stateMatch && request.method() === 'PUT') {
      const automationId = decodeURIComponent(stateMatch[1])
      const input = request.postDataJSON() as UpdateAutomationStateInput
      state.automationStateCommands.push(input)
      const index = state.automations.findIndex(
        (candidate) => candidate.id === automationId,
      )
      const current = state.automations[index]
      if (!current || current.version !== input.expected_version) {
        await route.fulfill({ status: 409 })
        return
      }
      const automation: Automation = {
        ...current,
        state: input.paused ? 'paused' : 'active',
        next_run_at_unix_ms: input.paused
          ? null
          : Date.now() + 86_400_000,
        version: String(Number(current.version) + 1),
        updated_at_unix_ms: Date.now(),
      }
      state.automations[index] = automation
      await route.fulfill({ json: { automation } })
      return
    }
    if (runsMatch && request.method() === 'GET') {
      const automationId = decodeURIComponent(runsMatch[1])
      await route.fulfill({
        json: { runs: state.automationRuns[automationId] ?? [] },
      })
      return
    }
    if (runsMatch && request.method() === 'POST') {
      const automationId = decodeURIComponent(runsMatch[1])
      const input = request.postDataJSON() as RunAutomationInput
      state.automationRunCommands.push(input)
      const automation = state.automations.find(
        (candidate) => candidate.id === automationId,
      )
      if (!automation || automation.version !== input.expected_version) {
        await route.fulfill({ status: 409 })
        return
      }
      const now = Date.now()
      const run: AutomationRun = {
        id: `run-${(state.automationRuns[automationId] ?? []).length + 1}`,
        automation_id: automationId,
        automation_version: automation.version,
        trigger: 'manual',
        scheduled_for_unix_ms: null,
        dispatch_command_id: `dispatch-${input.command_id}`,
        prompt_template: automation.prompt_template,
        selected_project_ids: [...automation.selected_project_ids],
        status: 'submitted',
        runtime_status: 'accepted',
        error_message: null,
        submitted_at_unix_ms: now,
        requested_by: input.actor,
        version: '1',
        created_at_unix_ms: now,
        updated_at_unix_ms: now,
      }
      state.automationRuns[automationId] = [
        run,
        ...(state.automationRuns[automationId] ?? []),
      ]
      automation.latest_run = run
      await route.fulfill({
        status: 201,
        json: {
          command_id: input.command_id,
          replayed: false,
          run,
        },
      })
      return
    }
    await route.fulfill({ status: 404 })
  })

  await page.route('**/api/v1/runtimes/herdr/**', async (route) => {
    const path = new URL(route.request().url()).pathname
    if (path.endsWith('/sessions')) {
      await options.sessionWait
      await route.fulfill({
        json: {
          adapter: sessions.adapter,
          sessions: state.runtimeSessions,
        },
      })
      return
    }
    if (
      route.request().method() === 'POST' &&
      path.endsWith('/open-ghostty')
    ) {
      const session = decodeURIComponent(path.split('/').at(-4) ?? '')
      const terminalId = decodeURIComponent(path.split('/').at(-2) ?? '')
      state.ghosttyRequests.push({ session, terminalId })
      if (options.ghosttyFails) {
        await route.fulfill({
          status: 503,
          json: {
            error: {
              code: 'ghostty_unavailable',
              message: 'Synthetic Ghostty unavailable',
            },
          },
        })
        return
      }
      await route.fulfill({
        json: {
          application: 'ghostty',
          terminal_id: terminalId,
          command: [
            'herdr',
            '--session',
            session,
            'agent',
            'attach',
            terminalId,
          ],
        },
      })
      return
    }
    if (path.includes('/sessions/beta/inventory') && options.betaFails) {
      await route.fulfill({
        status: 502,
        json: {
          error: {
            code: 'herdr_snapshot_failed',
            message: 'Synthetic beta snapshot failure',
          },
        },
      })
      return
    }
    state.inventoryRequests += 1
    const sessionMatch = path.match(/\/sessions\/([^/]+)\/inventory$/)
    const session = sessionMatch
      ? decodeURIComponent(sessionMatch[1])
      : state.runtimeInventory.session
    state.inventoryRequestSessions.push(session)
    const responsePlan = state.inventoryResponsePlans.get(session)?.shift()
    const observedAtUnixMs =
      responsePlan?.observedAtUnixMs ??
      state.runtimeInventory.observed_at_unix_ms
    state.requestLog.push(`inventory:${session}:start:${observedAtUnixMs}`)
    if (state.inventoryFailure) {
      await route.fulfill({
        status: 502,
        json: {
          error: {
            code: 'herdr_snapshot_failed',
            message: 'Synthetic inventory snapshot failure',
          },
        },
      })
      return
    }
    if (responsePlan?.reconcileRuntimeTimestamps) {
      reconcileTransferRuntimeTimestamps(
        state,
        session,
        observedAtUnixMs,
      )
    }
    await responsePlan?.wait
    if (
      options.reconcileWorkerOnInventory &&
      !state.workerCandidates.some(
        (candidate) => candidate.worker.id === 'worker-reconciled',
      )
    ) {
      state.workerCandidates.push({
        worker: durableWorker(
          'worker-reconciled',
          'terminal-9',
          state.profiles[0],
          'workspace-4',
        ),
        profile_name: state.profiles[0].name,
        default_role: state.profiles[0].default_role,
        availability: 'unassigned_live',
        reason: 'Discovered during runtime reconciliation.',
      })
    }
    state.requestLog.push(
      `inventory:${session}:complete:${observedAtUnixMs}`,
    )
    await route.fulfill({
      json: {
        ...state.runtimeInventory,
        observed_at_unix_ms: observedAtUnixMs,
        session,
      },
    })
  })

  return state
}

async function dragFirstProject(page: Page, deltaX: number, deltaY: number) {
  const territory = page.locator(
    '.projected-territory[data-node-id="project:project-1"] .territory-polygon',
  )
  const box = await territory.boundingBox()
  if (!box) throw new Error('Projected territory is not visible')
  // Whole-pixel start keeps the delivered pointer delta exactly (deltaX, deltaY)
  // once the browser rounds coordinates, which the projection assertions rely on.
  // This point is inside the parallelogram but clear of its upright label and
  // units, so the gesture exercises the projected territory's own hit target.
  const startX = Math.round(box.x + box.width * 0.25)
  const startY = Math.round(box.y + box.height / 2)
  await page.mouse.move(startX, startY)
  await page.mouse.down()
  await page.mouse.move(startX + deltaX, startY + deltaY, { steps: 8 })
  await page.mouse.up()
}

async function openSettings(page: Page) {
  const dialog = page.getByRole('dialog', { name: 'Settings' })
  if (!(await dialog.isVisible().catch(() => false))) {
    await page
      .getByRole('button', { name: /^Settings,/ })
      .click()
  }
  await expect(dialog).toBeVisible()
  return dialog
}

async function setMapView(page: Page, label: '2D view' | '2.5D view') {
  const dialog = await openSettings(page)
  await dialog.getByRole('button', { name: label }).click()
  await dialog.getByRole('button', { name: 'Close settings' }).click()
}

async function setAppTheme(page: Page, theme: 'Dark' | 'Light') {
  const dialog = await openSettings(page)
  await dialog.getByRole('button', { name: theme, exact: true }).click()
  await dialog.getByRole('button', { name: 'Close settings' }).click()
}

async function navigatorMetadataContrasts(navigator: Locator) {
  return navigator.evaluate((element) => {
    const selector = [
      ':scope > header small',
      '.agent-window-workspace__identity code',
      '.agent-window-workspace__identity small',
      '.agent-window-workspace__state span',
      '.agent-window-workspace__state b',
      '.agent-window-workspace__summary',
      '.agent-window-workspace__topology span',
      '.agent-window-workspace__worktree span',
      '.agent-window-workspace__worktree code',
      '.agent-window-row small',
      '.agent-window-row code',
      '[data-contrast-probe]',
    ].join(', ')
    const canvas = document.createElement('canvas')
    canvas.width = 1
    canvas.height = 1
    const context = canvas.getContext('2d', {
      willReadFrequently: true,
    })
    if (!context) throw new Error('Canvas color decoder is unavailable')

    const decode = (color: string) => {
      context.clearRect(0, 0, 1, 1)
      context.fillStyle = color
      context.fillRect(0, 0, 1, 1)
      const [red, green, blue, alpha] = context.getImageData(0, 0, 1, 1).data
      return [red, green, blue, alpha / 255] as const
    }
    const over = (
      foreground: readonly [number, number, number, number],
      background: readonly [number, number, number, number],
    ) => {
      const alpha =
        foreground[3] + background[3] * (1 - foreground[3])
      if (alpha === 0) return [0, 0, 0, 0] as const
      return [
        (foreground[0] * foreground[3] +
          background[0] * background[3] * (1 - foreground[3])) /
          alpha,
        (foreground[1] * foreground[3] +
          background[1] * background[3] * (1 - foreground[3])) /
          alpha,
        (foreground[2] * foreground[3] +
          background[2] * background[3] * (1 - foreground[3])) /
          alpha,
        alpha,
      ] as const
    }
    const compositedBackground = (node: Element) => {
      const ancestors: Element[] = []
      let current: Element | null = node
      while (current) {
        ancestors.push(current)
        current = current.parentElement
      }
      return ancestors
        .reverse()
        .reduce<readonly [number, number, number, number]>(
          (background, ancestor) =>
            over(
              decode(getComputedStyle(ancestor).backgroundColor),
              background,
            ),
          [255, 255, 255, 1],
        )
    }
    const luminance = (
      color: readonly [number, number, number, number],
    ) => {
      const channels = color.slice(0, 3).map((value) => value / 255)
      const [red, green, blue] = channels.map((channel) =>
        channel <= 0.04045
          ? channel / 12.92
          : ((channel + 0.055) / 1.055) ** 2.4,
      )
      return 0.2126 * red + 0.7152 * green + 0.0722 * blue
    }
    const contrast = (
      foreground: readonly [number, number, number, number],
      background: readonly [number, number, number, number],
    ) => {
      const foregroundLuminance = luminance(foreground)
      const backgroundLuminance = luminance(background)
      return (
        (Math.max(foregroundLuminance, backgroundLuminance) + 0.05) /
        (Math.min(foregroundLuminance, backgroundLuminance) + 0.05)
      )
    }

    return Array.from(element.querySelectorAll<HTMLElement>(selector))
      .filter((node) => {
        const style = getComputedStyle(node)
        return (
          style.display !== 'none' &&
          style.visibility !== 'hidden' &&
          node.getClientRects().length > 0
        )
      })
      .map((node) => {
        const authoredForeground = decode(getComputedStyle(node).color)
        const background = compositedBackground(node)
        const foreground = over(authoredForeground, background)
        return {
          background: background.slice(0, 3),
          contrast: contrast(foreground, background),
          foreground: foreground.slice(0, 3),
          foregroundAlpha: authoredForeground[3],
          label: `${node.className || node.tagName}: ${node.textContent?.trim()}`,
          sourceForeground: authoredForeground.slice(0, 3),
        }
      })
  })
}

async function openRuntimeHealth(page: Page) {
  const popover = page.getByRole('dialog', { name: 'Runtime health' })
  if (!(await popover.isVisible().catch(() => false))) {
    await page
      .getByRole('button', { name: /^Runtime health:/ })
      .click()
  }
  await expect(popover).toBeVisible()
  return popover
}

async function selectRuntimeSession(page: Page, session: string) {
  const popover = await openRuntimeHealth(page)
  await popover.getByLabel('Herdr session').selectOption(session)
  await page.keyboard.press('Escape')
}

function projectDropTarget(page: Page, index = 0) {
  return page
    .locator('.project-region')
    .nth(index)
    .locator('.workspace-region__heading')
}

async function dragToProject(
  source: Locator,
  page: Page,
  index = 0,
) {
  // Upright units can legitimately occupy the label's screen point. The
  // canvas resolves allocation drops from pointer geometry, so force only
  // skips Playwright's pre-drag hit-target check; it does not bypass Yard's
  // drag payload or drop handler.
  await source.dragTo(projectDropTarget(page, index), { force: true })
}

async function dragFirstWorkspace(
  page: Page,
  deltaX: number,
  deltaY: number,
) {
  const heading = page
    .locator(
      '.react-flow__node-workspace .workspace-region__heading',
    )
    .first()
  const box = await heading.boundingBox()
  if (!box) throw new Error('Workspace heading is not visible')
  const startX = box.x + box.width / 2
  const startY = box.y + box.height / 2
  await page.mouse.move(startX, startY)
  await page.mouse.down()
  await page.mouse.move(startX + deltaX, startY + deltaY, { steps: 8 })
  await page.mouse.up()
}

async function expectRuntimeLayout(page: Page) {
  const metrics = await page.evaluate(() => {
    const rect = (element: Element) => {
      const box = element.getBoundingClientRect()
      return {
        bottom: box.bottom,
        left: box.left,
        right: box.right,
        top: box.top,
      }
    }
    const projects = Array.from(
      document.querySelectorAll('.project-region'),
      rect,
    )
    const markers = Array.from(
      document.querySelectorAll<HTMLElement>('.worker-marker'),
    ).map((element, index) => ({
      id:
        element.dataset.workerId ??
        element.closest<HTMLElement>('.react-flow__node')?.dataset.id ??
        String(index),
      overflowX: element.scrollWidth - element.clientWidth,
      overflowY: element.scrollHeight - element.clientHeight,
      standalone:
        element
          .closest<HTMLElement>('.react-flow__node')
          ?.dataset.id?.startsWith('worker:') ?? false,
      ...rect(element),
    }))
    const outsideProject = markers
      .filter(
        (marker) =>
          !marker.standalone &&
          !projects.some(
            (project) =>
              marker.left >= project.left - 1 &&
              marker.right <= project.right + 1 &&
              marker.top >= project.top - 1 &&
              marker.bottom <= project.bottom + 1,
          ),
      )
      .map((marker) => marker.id)
    const overlaps: string[] = []
    for (let left = 0; left < markers.length; left += 1) {
      for (let right = left + 1; right < markers.length; right += 1) {
        const a = markers[left]
        const b = markers[right]
        const overlapWidth = Math.min(a.right, b.right) - Math.max(a.left, b.left)
        const overlapHeight =
          Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top)
        if (overlapWidth > 1 && overlapHeight > 1) {
          overlaps.push(`${a.id}:${b.id}`)
        }
      }
    }

    return {
      documentOverflowX: document.documentElement.scrollWidth - window.innerWidth,
      documentOverflowY:
        document.documentElement.scrollHeight - window.innerHeight,
      markerOverflowX: Math.max(0, ...markers.map((marker) => marker.overflowX)),
      markerOverflowY: Math.max(0, ...markers.map((marker) => marker.overflowY)),
      outsideProject,
      overlaps,
    }
  })

  expect(metrics.documentOverflowX).toBeLessThanOrEqual(0)
  expect(metrics.documentOverflowY).toBeLessThanOrEqual(0)
  // The depth-mode sprite is deliberately larger than its marker's real,
  // click-matched hit region (see the `.worker-marker__body
  // .worker-marker__sprite { inset: -53px }` rule in App.css) so the
  // character reads at a usable size without growing the actual pointer
  // target into a dead zone that steals clicks from whatever's nearby. That
  // overflowing, pointer-events:none sprite still counts toward its
  // ancestor's scrollWidth/scrollHeight even though it never affects hit
  // testing or document layout — this bound tolerates that specific,
  // intentional overflow (measured ~50px in practice) without opening the
  // door to unbounded marker growth elsewhere.
  expect(metrics.markerOverflowX).toBeLessThanOrEqual(60)
  expect(metrics.markerOverflowY).toBeLessThanOrEqual(60)
  expect(metrics.outsideProject).toEqual([])
  expect(metrics.overlaps).toEqual([])
}

function seedActiveAssignment(
  state: MockState,
  id = 'assignment-1',
) {
  const seeded = assignment(
    id,
    'project-1',
    state.profiles[0],
    'Inspect and guide the active work.',
    'implementer',
  )
  state.assignments.push(seeded)
  return seeded
}

function seedAssignedCandidateAssignment(state: MockState) {
  const candidate = state.workerCandidates.find(
    (worker) => worker.worker.id === 'worker-assigned',
  )
  if (!candidate) throw new Error('Assigned worker fixture is missing')
  const seeded = assignment(
    candidate.assignment_id ?? 'assignment-active',
    candidate.project_id ?? 'project-1',
    state.profiles[0],
    'Continue the active implementation.',
    'implementer',
    candidate.worker,
  )
  state.assignments.push(seeded)
  return seeded
}

test('uses wss for terminal URLs on secure pages', () => {
  const location = {
    href: 'https://yard.example/control',
    protocol: 'https:',
  }
  const assignmentUrl = assignmentTerminalWebSocketUrl(
    'project/secure',
    'assignment secure',
    100,
    40,
    location,
  )
  const orchestratorUrl = orchestratorTerminalWebSocketUrl(
    'project/secure',
    90,
    30,
    location,
  )

  expect(assignmentUrl).toBe(
    'wss://yard.example/api/v1/projects/project%2Fsecure/assignments/assignment%20secure/terminal?cols=100&rows=40',
  )
  expect(orchestratorUrl).toBe(
    'wss://yard.example/api/v1/projects/project%2Fsecure/orchestrator/terminal?cols=90&rows=30',
  )
})

test('renders durable, offline, and unbound resources on desktop', async ({
  page,
}, testInfo) => {
  const state = await mockApi(page)
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto('/')

  await expect(
    page.locator('.brand').getByText('Yard', { exact: true }),
  ).toBeVisible()
  await expect(page.locator('.project-region')).toHaveCount(2)
  await expect(
    page.locator('.project-region[data-runtime="online"]'),
  ).toHaveCount(1)
  await expect(
    page.locator('.project-region[data-runtime="offline"]'),
  ).toHaveCount(1)
  await expect(page.locator('.worker-marker')).toHaveCount(
    state.runtimeInventory.workers.length + 1,
  )
  await expect(page.locator('.orchestrator-marker')).toHaveCount(2)

  await page.getByRole('tab', { name: 'Workspaces' }).click()
  await expect(page.locator('.workspace-row')).toHaveCount(3)

  const overflow = await page.evaluate(() => ({
    horizontal: document.documentElement.scrollWidth - window.innerWidth,
    vertical: document.documentElement.scrollHeight - window.innerHeight,
  }))
  expect(overflow.horizontal).toBeLessThanOrEqual(0)
  expect(overflow.vertical).toBeLessThanOrEqual(0)

  await page.screenshot({
    path: testInfo.outputPath('desktop.png'),
    fullPage: true,
  })
})

test('keeps the canvas visible under a compact collapsible resource shelf', async ({
  page,
}) => {
  await mockApi(page)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  const commandBar = page.locator('.command-bar')
  const shelf = page.locator('.resource-shelf')
  const canvas = page.locator('.canvas-stage')
  const createProject = page.getByRole('button', {
    name: 'Create project',
    exact: true,
  })
  const resourceTabs = page.getByRole('tablist', {
    name: 'Observed resources',
  })

  const [commandBounds, shelfBounds, canvasBounds, createBounds, tabsBounds] =
    await Promise.all([
      commandBar.boundingBox(),
      shelf.boundingBox(),
      canvas.boundingBox(),
      createProject.boundingBox(),
      resourceTabs.boundingBox(),
    ])
  expect(commandBounds?.height).toBeLessThanOrEqual(48)
  expect(shelfBounds?.height).toBeLessThanOrEqual(96)
  expect(createBounds?.x ?? Infinity).toBeLessThan(tabsBounds?.x ?? 0)
  expect(canvasBounds?.height ?? 0).toBeGreaterThan(600)

  await page
    .getByRole('button', { name: 'Collapse resource shelf' })
    .click()
  await expect(shelf).toHaveCount(0)
  await expect(page.locator('.project-region').first()).toBeVisible()
  await expect
    .poll(async () => (await canvas.boundingBox())?.height ?? 0)
    .toBeGreaterThan(canvasBounds?.height ?? 0)

  await page.getByRole('tab', { name: 'Profiles' }).click()
  await expect(shelf).toBeVisible()
})

test('provides one-click runtime health and persistent appearance settings', async ({
  page,
}, testInfo) => {
  await page.addInitScript(() => {
    if (window.localStorage.getItem('yard:theme') === null) {
      window.localStorage.setItem('yard:theme', 'light')
    }
    if (window.localStorage.getItem('yard:map-visual-mode:v1') === null) {
      window.localStorage.setItem('yard:map-visual-mode:v1', 'depth')
    }
  })
  await mockApi(page)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  const commandBar = page.locator('.command-bar')
  const healthTrigger = page.getByRole('button', {
    name: /^Runtime health:/,
  })
  const settingsTrigger = page.getByRole('button', {
    name: /^Settings,/,
  })
  await expect(commandBar).toBeVisible()
  expect((await commandBar.boundingBox())?.height).toBeLessThanOrEqual(48)
  expect(
    await commandBar.evaluate(
      (element) => element.scrollWidth - element.clientWidth,
    ),
  ).toBeLessThanOrEqual(0)
  await expect(healthTrigger).toBeVisible()
  await expect(healthTrigger).toHaveAccessibleName(
    'Runtime health: alpha, Herdr observed',
  )
  await expect(settingsTrigger).toHaveAccessibleName(
    'Settings, light theme, 2.5D map',
  )
  await expect(page.locator('.command-bar__metrics')).toHaveCount(0)
  await expect(page.locator('.canvas-stage__label')).toHaveCount(0)
  await expect(page.locator('.canvas-view-switcher')).toHaveCount(0)

  await healthTrigger.focus()
  await healthTrigger.press('Enter')
  const health = page.getByRole('dialog', { name: 'Runtime health' })
  const sessionSelect = health.getByLabel('Herdr session')
  await expect(sessionSelect).toBeFocused()
  await expect(health.getByText('Herdr observed')).toBeVisible()
  await page.screenshot({
    path: testInfo.outputPath('slice1-runtime-health-desktop.png'),
    fullPage: true,
  })
  await sessionSelect.selectOption('beta')
  await expect(
    page.getByRole('button', {
      name: /Runtime health: beta, Herdr observed/,
    }),
  ).toBeVisible()
  await page.keyboard.press('Escape')
  await expect(health).toHaveCount(0)
  await expect(healthTrigger).toBeFocused()

  await healthTrigger.press('Enter')
  await page.locator('.canvas-stage').click({ position: { x: 8, y: 80 } })
  await expect(health).toHaveCount(0)
  await expect(healthTrigger).toBeFocused()

  await settingsTrigger.focus()
  await settingsTrigger.press('Enter')
  const settings = page.getByRole('dialog', { name: 'Settings' })
  await expect(
    settings.getByRole('button', { name: 'Light', exact: true }),
  ).toBeFocused()
  await expect(page.locator('.app-shell')).toHaveAttribute('inert', '')
  for (let index = 0; index < 12; index += 1) {
    await page.keyboard.press('Tab')
    expect(
      await settings.evaluate((element) =>
        element.contains(document.activeElement),
      ),
    ).toBe(true)
  }
  await settings
    .getByRole('button', { name: 'Dark', exact: true })
    .click()
  await settings.getByRole('button', { name: '2D view' }).click()
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark')
  await expect(page.getByLabel('Yard project canvas')).toHaveAttribute(
    'data-visual-mode',
    'flat',
  )
  expect(
    await page.evaluate(() => ({
      map: window.localStorage.getItem('yard:map-visual-mode:v1'),
      theme: window.localStorage.getItem('yard:theme'),
    })),
  ).toEqual({ map: 'flat', theme: 'dark' })
  await page.screenshot({
    path: testInfo.outputPath('slice1-settings-desktop.png'),
    fullPage: true,
  })
  await page.keyboard.press('Escape')
  await expect(settings).toHaveCount(0)
  await expect(settingsTrigger).toBeFocused()

  await page.reload()
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark')
  await expect(page.getByLabel('Yard project canvas')).toHaveAttribute(
    'data-visual-mode',
    'flat',
  )
  await page.screenshot({
    path: testInfo.outputPath('slice1-command-bar-desktop.png'),
    fullPage: true,
  })
})

test('keeps Slice 1 chrome visible and motion-safe at mobile widths', async ({
  page,
}, testInfo) => {
  await mockApi(page)
  await page.emulateMedia({ reducedMotion: 'reduce' })

  for (const width of [390, 320]) {
    await page.setViewportSize({ width, height: 844 })
    await page.goto('/')

    const commandBar = page.locator('.command-bar')
    const healthTrigger = page.getByRole('button', {
      name: /Runtime health: alpha, Herdr observed/,
    })
    await expect(healthTrigger).toBeVisible()
    expect((await commandBar.boundingBox())?.height).toBeLessThanOrEqual(46)
    const overflow = await page.evaluate(() => {
      const bar = document.querySelector<HTMLElement>('.command-bar')
      return {
        bar: bar ? bar.scrollWidth - bar.clientWidth : Number.NaN,
        document:
          document.documentElement.scrollWidth -
          document.documentElement.clientWidth,
      }
    })
    expect(overflow.bar).toBeLessThanOrEqual(0)
    expect(overflow.document).toBeLessThanOrEqual(0)

    const health = await openRuntimeHealth(page)
    const healthBounds = await health.boundingBox()
    expect(healthBounds?.x ?? -1).toBeGreaterThanOrEqual(0)
    expect(
      healthBounds
        ? healthBounds.x + healthBounds.width
        : Number.POSITIVE_INFINITY,
    ).toBeLessThanOrEqual(width)
    expect(
      await health.evaluate(
        (element) => getComputedStyle(element).animationName,
      ),
    ).toBe('none')
    await page.keyboard.press('Escape')

    const settings = await openSettings(page)
    const settingsBounds = await settings.boundingBox()
    expect(settingsBounds?.x ?? -1).toBeGreaterThanOrEqual(0)
    expect(
      settingsBounds
        ? settingsBounds.x + settingsBounds.width
        : Number.POSITIVE_INFINITY,
    ).toBeLessThanOrEqual(width)
    expect(
      await settings.evaluate(
        (element) => getComputedStyle(element).animationName,
      ),
    ).toBe('none')
    await settings.getByRole('button', { name: '2D view' }).click()
    await expect(page.getByLabel('Yard project canvas')).toHaveAttribute(
      'data-visual-mode',
      'flat',
    )
    await settings.getByRole('button', { name: 'Close settings' }).click()

    await page.screenshot({
      path: testInfo.outputPath(`slice1-command-bar-${width}.png`),
      fullPage: true,
    })
  }
})

test('renders observed agents on the canvas before a project exists', async ({
  page,
}) => {
  const state = await mockApi(page)
  state.projects = []
  state.assignments = []
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto('/')

  await expect(
    page.getByText('Herdr observed', { exact: true }),
  ).toBeVisible()
  await expect(page.locator('.project-region')).toHaveCount(0)
  await expect(page.locator('.worker-marker')).toHaveCount(
    state.runtimeInventory.workers.length,
  )
  await expect(page.locator('.canvas-empty')).toHaveCount(0)

  await page.locator('.worker-marker').first().click()
  await expect(page.locator('.inspector h2')).not.toBeEmpty()
})

test('renders provider child agents beneath their terminal-backed parent', async ({
  page,
}) => {
  const state = await mockApi(page)
  state.projects[0].placement.geometry.x = -120
  const parentSession = state.runtimeInventory.workers[0].provider_session
  expect(parentSession).not.toBeNull()
  state.runtimeInventory.child_agents = [
    {
      runtime_id: 'provider-child:codex:synthetic-session-1:rawls',
      parent_provider_session: parentSession!,
      parent_agent_id: null,
      provider: 'codex',
      provider_agent_id: 'rawls',
      name: 'Rawls',
      description: null,
      role: 'explorer',
      status: 'working',
      depth: 1,
      updated_at_unix_ms: Date.now(),
    },
    {
      runtime_id: 'provider-child:codex:synthetic-session-1:aristotle',
      parent_provider_session: parentSession!,
      parent_agent_id: 'rawls',
      provider: 'codex',
      provider_agent_id: 'aristotle',
      name: 'Aristotle',
      description: null,
      role: 'reviewer',
      status: 'working',
      depth: 2,
      updated_at_unix_ms: Date.now(),
    },
    {
      runtime_id: 'provider-child:codex:synthetic-session-1:finished',
      parent_provider_session: parentSession!,
      parent_agent_id: null,
      provider: 'codex',
      provider_agent_id: 'finished',
      name: 'Finished',
      description: null,
      role: 'explorer',
      status: 'done',
      depth: 1,
      updated_at_unix_ms: Date.now(),
    },
  ]
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto('/')

  await expect(page.locator('.child-agent-marker')).toHaveCount(2)
  await expect(page.locator('.provider-child-edge')).toHaveCount(2)
  await expect(page.getByText('Finished', { exact: true })).toHaveCount(0)
  // Agent markers are billboards standing on projected ground anchors in 2.5D,
  // so their box is the drawn sprite scaled to the ground plane. 2D view keeps
  // the original flat card footprint.
  const billboard = await page
    .locator('.react-flow__node-worker')
    .first()
    .evaluate((element) => ({
      height: Number.parseFloat((element as HTMLElement).style.height),
      width: Number.parseFloat((element as HTMLElement).style.width),
    }))
  expect(billboard.width).toBeCloseTo(101 * 0.78, 3)
  expect(billboard.height).toBeCloseTo(115 * 0.78, 3)
  await setMapView(page, '2D view')
  await expect(page.locator('.react-flow__node-worker').first()).toHaveCSS(
    'width',
    '101px',
  )
  await expect(page.locator('.react-flow__node-worker').first()).toHaveCSS(
    'height',
    '115px',
  )
  await expect(page.locator('.react-flow__node-child-agent').first()).toHaveCSS(
    'width',
    '83px',
  )
  await expect(page.locator('.react-flow__node-child-agent').first()).toHaveCSS(
    'height',
    '92px',
  )
  await setMapView(page, '2.5D view')

  const projectNode = page.locator('[data-id="project:project-1"]')
  const yardOrchestratorNode = page.locator('[data-id="yard-orchestrator"]')
  const [projectBox, yardOrchestratorBox] = await Promise.all([
    projectNode.boundingBox(),
    yardOrchestratorNode.boundingBox(),
  ])
  expect(projectBox).not.toBeNull()
  expect(yardOrchestratorBox).not.toBeNull()
  expect(
    (yardOrchestratorBox?.x ?? 0) + (yardOrchestratorBox?.width ?? 0),
  ).toBeLessThanOrEqual(projectBox?.x ?? 0)

  const rawlsMarker = page.getByRole('button', {
    name: /Rawls, codex child agent/,
  })
  const rootParentId = await rawlsMarker.getAttribute(
    'data-parent-node-id',
  )
  expect(rootParentId).not.toBeNull()
  const parentNode = page.locator(
    `.react-flow__node[data-id="${rootParentId}"]`,
  )
  const rawlsNode = page.locator(
    '[data-id="child-agent:provider-child:codex:synthetic-session-1:rawls"]',
  )
  const aristotleNode = page.locator(
    '[data-id="child-agent:provider-child:codex:synthetic-session-1:aristotle"]',
  )
  const [parentBox, rawlsBox, aristotleBox] = await Promise.all([
    parentNode.boundingBox(),
    rawlsNode.boundingBox(),
    aristotleNode.boundingBox(),
  ])
  expect(parentBox).not.toBeNull()
  expect(rawlsBox).not.toBeNull()
  expect(aristotleBox).not.toBeNull()
  expect(rawlsBox?.x ?? 0).toBeGreaterThan(
    (parentBox?.x ?? 0) + (parentBox?.width ?? 0),
  )
  expect(aristotleBox?.x ?? 0).toBeGreaterThan(
    (rawlsBox?.x ?? 0) + (rawlsBox?.width ?? 0),
  )
  await expect(rawlsMarker).toHaveAttribute(
    'data-parent-node-id',
    rootParentId!,
  )
  await expect(
    page.getByRole('button', { name: /Aristotle, codex child agent/ }),
  ).toHaveAttribute(
    'data-parent-node-id',
    'child-agent:provider-child:codex:synthetic-session-1:rawls',
  )

  const edgePaths = page.locator(
    '.provider-child-edge .react-flow__edge-path',
  )
  const edgeRendering = await edgePaths.evaluateAll((paths) =>
    paths.map((path) => {
      const style = getComputedStyle(path)
      return {
        length: (path as SVGPathElement).getTotalLength(),
        path: path.getAttribute('d'),
        stroke: style.stroke,
        strokeWidth: Number.parseFloat(style.strokeWidth),
      }
    }),
  )
  expect(
    edgeRendering.every(
      (edge) =>
        Boolean(edge.path) &&
        edge.length > 4 &&
        edge.stroke !== 'none' &&
        edge.strokeWidth >= 3,
    ),
  ).toBe(true)

  // A unit can legitimately stand over the billboard label in the projected
  // view. Force selection through the label here so this visual assertion does
  // not depend on whichever unit happens to overlap that exact screen point.
  await projectNode.locator('.workspace-region__heading').click({ force: true })
  await rawlsMarker.click()
  await expect(page.locator('.inspector h2')).toHaveText('Rawls')
  await expect(page.getByText('Shares parent terminal')).toBeVisible()
})

test('keeps raised worker units stable and 2D motion respects reduced motion', async ({
  page,
}) => {
  await mockApi(page)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  const marker = page.locator('.worker-marker__motion').first()
  await expect(marker).toBeVisible()
  expect(
    await marker.evaluate(
      (element) => getComputedStyle(element).animationName,
    ),
  ).toBe('none')
  await setMapView(page, '2D view')
  expect(
    await marker.evaluate(
      (element) => getComputedStyle(element).animationName,
    ),
  ).toBe('worker-drift')
  expect(
    await marker.evaluate((element) =>
      Number.parseFloat(getComputedStyle(element).animationDuration),
    ),
  ).toBeGreaterThan(0)

  await page.emulateMedia({ reducedMotion: 'reduce' })
  await page.reload()
  expect(
    await page
      .locator('.worker-marker__motion')
      .first()
      .evaluate((element) => getComputedStyle(element).animationName),
  ).toBe('none')
})

test('keeps exactly one durable orchestrator visible across selected sessions', async ({
  page,
}, testInfo) => {
  const state = await mockApi(page)
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto('/')

  const orchestrators = page.locator('.orchestrator-marker')
  await expect(orchestrators).toHaveCount(2)
  await expect(
    page.locator(
      '.orchestrator-marker[data-project-id="project-1"][data-observation-state="observed"]',
    ),
  ).toHaveCount(1)
  const offlineOrchestrator = page.locator(
    '.orchestrator-marker[data-project-id="project-2"]',
  )
  await expect(offlineOrchestrator).toHaveCount(1)
  await expect(offlineOrchestrator).toHaveAttribute(
    'data-observation-state',
    'missing',
  )
  await expect(offlineOrchestrator).toHaveAttribute(
    'data-process-state',
    'unknown',
  )
  await expect(
    page.locator('.worker-marker[data-role="worker"]'),
  ).toHaveCount(state.runtimeInventory.workers.length - 1)
  await expectRuntimeLayout(page)

  await selectRuntimeSession(page, 'beta')
  await expect(
    page.locator('.project-region[data-runtime="offline"]'),
  ).toHaveCount(2)
  await expect(orchestrators).toHaveCount(2)
  await expect(
    page.locator('.worker-marker[data-role="worker"]'),
  ).toHaveCount(state.runtimeInventory.workers.length)

  await offlineOrchestrator.click()
  const inspector = page.locator('.inspector')
  const runtimeSection = inspector.getByRole('region', {
    name: 'Orchestrator runtime',
  })
  await expect(runtimeSection).toBeVisible()
  await expect(
    runtimeSection.locator(
      '.runtime-state-summary[data-current-observation="false"] .status-badge[data-status="unknown"]',
    ),
  ).toBeVisible()
  await expect(
    runtimeSection.locator(
      '.process-state-badge[data-process-state="unknown"]',
    ),
  ).toBeVisible()
  await expect(
    runtimeSection.locator(
      '.observation-state-badge[data-observation-state="missing"]',
    ),
  ).toBeVisible()
  await expect(
    runtimeSection.locator('.detail-row').filter({ hasText: 'Worker ID' }),
  ).toContainText('project-2-orchestrator')
  await expect(
    runtimeSection.locator('.detail-row').filter({ hasText: 'Herdr session' }),
  ).toContainText('gamma')
  await expect(
    runtimeSection.locator('.detail-row').filter({ hasText: 'Terminal' }),
  ).toContainText('terminal-offline')

  await page.screenshot({
    path: testInfo.outputPath('offline-orchestrator-desktop.png'),
    fullPage: true,
  })

  await page.setViewportSize({ width: 390, height: 844 })
  await page.reload()
  await expect(orchestrators).toHaveCount(2)
  await expectRuntimeLayout(page)
  await page.screenshot({
    path: testInfo.outputPath('durable-orchestrators-mobile.png'),
    fullPage: true,
  })
})

test('projects active assignments once and historical live workers as observed', async ({
  page,
}, testInfo) => {
  const state = await mockApi(page)
  const active = seedAssignedCandidateAssignment(state)
  const historical = [
    {
      lifecycle: 'completed' as const,
      terminalId: 'terminal-4',
      prompt: 'Retain the completed worker until its session ends.',
    },
    {
      lifecycle: 'failed' as const,
      terminalId: 'terminal-5',
      prompt: 'Keep the failed worker available for diagnosis.',
    },
  ].map(({ lifecycle, terminalId, prompt }) => {
    const historicalAssignment = assignment(
      `assignment-${lifecycle}-live`,
      'project-1',
      state.profiles[0],
      prompt,
      'implementer',
      durableWorker(
        `worker-${lifecycle}-live`,
        terminalId,
        state.profiles[0],
      ),
    )
    historicalAssignment.lifecycle = lifecycle
    historicalAssignment.attempt.lifecycle = lifecycle
    if (lifecycle === 'completed') {
      historicalAssignment.completion_receipt = {
        id: 'receipt-completed-live',
        assignment_id: historicalAssignment.id,
        attempt_id: historicalAssignment.attempt.id,
        outcome: 'completed',
        summary: 'The assignment is complete while the terminal remains live.',
        artifact_refs: [],
        artifacts: [],
        evidence_refs: [],
        unresolved_blockers: [],
        actor: 'local-user',
        created_at_unix_ms: Date.now(),
      }
    } else {
      historicalAssignment.attempt.error = 'Synthetic historical failure.'
    }
    state.assignments.push(historicalAssignment)
    return historicalAssignment
  })
  const activeTerminalId = active.worker.runtime?.terminal_id
  if (
    !activeTerminalId ||
    historical.some(({ worker }) => !worker.runtime?.terminal_id)
  ) {
    throw new Error('Projection fixtures require terminal-backed workers')
  }
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  const activeMarker = page.locator(
    `.assigned-worker-marker[data-worker-id="${active.worker.id}"]`,
  )
  const activeObservedNode = page.locator(
    `.react-flow__node-worker[data-id="worker:${activeTerminalId}"]`,
  )
  const activeSprite = activeMarker.locator('.worker-marker__sprite')
  const expectProjection = async () => {
    await expect(activeMarker).toHaveCount(1)
    await expect(activeSprite).toBeVisible()
    await expect(activeSprite.locator('svg[data-crew-role="worker"]')).toBeVisible()
    await expect(activeObservedNode).toHaveCount(0)

    for (const historicalAssignment of historical) {
      const terminalId = historicalAssignment.worker.runtime!.terminal_id
      const marker = page.locator(
        `.assigned-worker-marker[data-worker-id="${historicalAssignment.worker.id}"]`,
      )
      const observedNode = page.locator(
        `.react-flow__node-worker[data-id="worker:${terminalId}"]`,
      )
      await expect(marker).toHaveCount(0)
      await expect(observedNode).toHaveCount(1)
      await expect(observedNode).toBeVisible()
    }
  }

  await expectProjection()
  await page.reload()
  await expectProjection()
  await page.screenshot({
    path: testInfo.outputPath('worker-projection-active-historical.png'),
    fullPage: true,
  })
})

test('scopes terminal projection identity to adapter and session', async ({
  page,
}) => {
  const state = await mockApi(page)
  const collidingAssignment = assignment(
    'assignment-beta-terminal-collision',
    'project-1',
    state.profiles[0],
    'Continue work in the beta Herdr session.',
    'implementer',
    durableWorker(
      'worker-beta-terminal-collision',
      'terminal-2',
      state.profiles[0],
      'workspace-1',
      'beta',
      true,
      { status: 'blocked' },
    ),
  )
  state.assignments.push(collidingAssignment)
  const collidingCandidate: WorkerCandidate = {
    worker: collidingAssignment.worker,
    profile_name: state.profiles[0].name,
    default_role: state.profiles[0].default_role,
    availability: 'assigned',
    project_id: collidingAssignment.project_id,
    assignment_id: collidingAssignment.id,
  }
  state.workerCandidates.unshift(collidingCandidate)
  state.workerCandidates.push({
    worker: durableWorker(
      'worker-beta-resumable-collision',
      'terminal-2',
      state.profiles[0],
      'workspace-1',
      'beta',
    ),
    profile_name: state.profiles[0].name,
    default_role: state.profiles[0].default_role,
    availability: 'resumable',
    assignment_id: 'assignment-beta-resumable-collision',
    reason: 'Foreign-session collision fixture.',
  })
  const foreignAdapterWorker = durableWorker(
    'worker-foreign-adapter-collision',
    'terminal-2',
    state.profiles[0],
  )
  if (!foreignAdapterWorker.runtime) {
    throw new Error('Adapter collision fixture requires a runtime')
  }
  foreignAdapterWorker.runtime.adapter = 'foreign-runtime'
  state.workerCandidates.push({
    worker: foreignAdapterWorker,
    profile_name: state.profiles[0].name,
    default_role: state.profiles[0].default_role,
    availability: 'resumable',
    assignment_id: 'assignment-foreign-adapter-collision',
    reason: 'Foreign-adapter collision fixture.',
  })
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await expect(
    page.locator(
      '.assigned-worker-marker[data-worker-id="worker-beta-terminal-collision"]',
    ),
  ).toHaveAttribute('data-status', 'blocked')
  await expect(
    page.locator(
      '.react-flow__node-worker[data-id="worker:terminal-2"]',
    ),
  ).toHaveCount(1)

  const alphaWorker = page.locator(
    '.react-flow__node-worker[data-id="worker:terminal-2"]',
  )
  await alphaWorker.click()
  await expect(
    page
      .locator('.inspector .detail-row')
      .filter({ hasText: 'Worker ID' }),
  ).toContainText('worker-unassigned')
  await expect(
    page.getByRole('button', { name: 'Assign worker', exact: true }),
  ).toBeVisible()
  await expect(
    page.getByRole('button', { name: 'Hand off worker', exact: true }),
  ).toHaveCount(0)

  await dragToProject(alphaWorker.locator('.worker-marker'), page, 1)
  const dialog = page.getByRole('dialog', { name: 'Assign worker' })
  await expect(dialog).toBeVisible()
  await expect(
    page.getByRole('dialog', { name: 'Resume worker' }),
  ).toHaveCount(0)
  await expect(dialog.getByLabel('Worker profile')).toHaveValue('')
  await dialog
    .getByLabel('Objective')
    .fill('Keep the alpha runtime identity scoped during allocation.')
  await dialog
    .getByRole('button', { name: 'Assign worker', exact: true })
    .click()
  const allocation = state.allocationCommands.at(-1)
  if (!allocation || !('worker_id' in allocation)) {
    throw new Error('Expected a live-worker allocation command')
  }
  expect(state.allocationCommands).toHaveLength(1)
  expect(allocation.worker_id).toBe('worker-unassigned')
})

test('uses durable status for missing ambiguous and exited worker candidates', async ({
  page,
}) => {
  const state = await mockApi(page)
  state.runtimeInventory.workers = state.runtimeInventory.workers.filter(
    (candidate) => candidate.terminal_id !== 'terminal-7',
  )
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')
  await page.getByRole('tab', { name: 'Workers' }).click()

  const inspector = page.locator('.inspector')
  const resumable = page.locator(
    '.worker-row[data-worker-id="worker-resumable"]',
  )
  await expect(resumable).toHaveAttribute('data-status', 'idle')
  await expect(resumable).toHaveAttribute('data-process-state', 'exited')
  await expect(resumable).toContainText('Resumable / idle')
  await resumable.click()
  await expect(
    inspector.locator('.status-badge[data-status="idle"]'),
  ).toBeVisible()
  await expect(
    inspector.locator('.process-state-badge[data-process-state="exited"]'),
  ).toBeVisible()
  await expect(
    inspector.locator(
      '.observation-state-badge[data-observation-state="observed"]',
    ),
  ).toBeVisible()
  await expect(
    inspector.locator('.status-badge[data-status="done"]'),
  ).toHaveCount(0)

  await page
    .locator('.worker-row[data-worker-id="worker-unavailable"]')
    .click()
  await expect(
    inspector.locator(
      '.observation-state-badge[data-observation-state="missing"]',
    ),
  ).toBeVisible()
  await expect(
    inspector.locator('.status-badge[data-status="unknown"]'),
  ).toBeVisible()

  await page.locator('.worker-row[data-worker-id="worker-ambiguous"]').click()
  await expect(
    inspector.locator(
      '.observation-state-badge[data-observation-state="ambiguous"]',
    ),
  ).toBeVisible()
  await expect(
    inspector.locator('.process-state-badge[data-process-state="unknown"]'),
  ).toBeVisible()

  await page.setViewportSize({ width: 390, height: 844 })
  await expect(inspector).toBeVisible()
  const overflow = await inspector.evaluate((element) => ({
    horizontal: element.scrollWidth - element.clientWidth,
    right: element.getBoundingClientRect().right - window.innerWidth,
  }))
  expect(overflow.horizontal).toBeLessThanOrEqual(0)
  expect(overflow.right).toBeLessThanOrEqual(0)
})

test('keeps exited assignment process state separate from runtime status', async ({
  page,
}) => {
  const state = await mockApi(page)
  const seeded = seedActiveAssignment(state)
  if (!seeded.worker.runtime) throw new Error('Seeded worker must have a runtime')
  seeded.worker.runtime.process_state = 'exited'
  seeded.worker.runtime.status = 'blocked'

  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  const marker = page.locator('.assigned-worker-marker')
  await expect(marker).toHaveAttribute('data-status', 'blocked')
  await expect(marker).toHaveAttribute('data-process-state', 'exited')
  await expect(
    page.locator('.assigned-worker-marker[data-status="done"]'),
  ).toHaveCount(0)
  await marker.click()

  const inspector = page.locator('.inspector')
  await expect(
    inspector.locator('.status-badge[data-status="blocked"]'),
  ).toBeVisible()
  await expect(
    inspector.locator('.process-state-badge[data-process-state="exited"]'),
  ).toBeVisible()
  await expect(inspector.locator('.runtime-badge')).toContainText('active')
  await expect(
    inspector.getByRole('button', { name: 'Record completion' }),
  ).toBeVisible()
})

test('keeps Herdr in a loading state until session discovery completes', async ({
  page,
}) => {
  let releaseSessions: () => void = () => undefined
  const sessionWait = new Promise<void>((resolve) => {
    releaseSessions = resolve
  })
  await mockApi(page, { sessionWait })
  await page.setViewportSize({ width: 1200, height: 760 })
  await page.goto('/')

  await expect(
    page.getByRole('button', { name: /Observing Herdr/ }),
  ).toBeVisible()
  await expect(
    page.getByRole('button', { name: /Herdr unavailable/ }),
  ).toHaveCount(0)
  await expect(page.locator('.project-region')).toHaveCount(2)
  await expect(
    page.locator('.project-region[data-runtime="loading"]'),
  ).toHaveCount(2)
  await expect(
    page.locator('.project-region[data-runtime="offline"]'),
  ).toHaveCount(0)

  releaseSessions()
  await expect(page.locator('.worker-marker')).toHaveCount(
    workers.length + 1,
  )
  await expect(
    page.getByRole('button', { name: /Herdr observed/ }),
  ).toBeVisible()
  await expect(
    page.locator('.project-region[data-runtime="loading"]'),
  ).toHaveCount(0)
  await expect(
    page.locator('.project-region[data-runtime="online"]'),
  ).toHaveCount(1)
  await expect(
    page.locator('.project-region[data-runtime="offline"]'),
  ).toHaveCount(1)
})

test('resnapshots inventory, retains stale state, and recovers after failure', async ({
  page,
}) => {
  const state = await mockApi(page)
  await page.setViewportSize({ width: 1200, height: 760 })
  await page.goto('/')

  await expect(page.locator('.worker-marker')).toHaveCount(
    workers.length + 1,
  )
  const initialRequests = state.inventoryRequests
  state.runtimeInventory = {
    ...state.runtimeInventory,
    observed_at_unix_ms: Date.now(),
    workers: [
      ...state.runtimeInventory.workers,
      worker(10, 'workspace-1', 'working'),
    ],
  }

  await expect
    .poll(() => state.inventoryRequests, { timeout: 3_000 })
    .toBeGreaterThan(initialRequests)
  await expect(page.locator('.worker-marker')).toHaveCount(
    workers.length + 2,
  )
  await page.getByText('worker-10', { exact: true }).click()
  await expect(page.locator('.inspector .status-badge')).toHaveAttribute(
    'data-status',
    'working',
  )

  const requestsBeforeFailure = state.inventoryRequests
  state.inventoryFailure = true
  await expect
    .poll(() => state.inventoryRequests, { timeout: 3_000 })
    .toBeGreaterThan(requestsBeforeFailure)
  await expect(
    page.getByText('Synthetic inventory snapshot failure'),
  ).toBeVisible()
  await expect(page.locator('.worker-marker')).toHaveCount(
    workers.length + 2,
  )
  await expect(page.locator('.inspector h2')).toHaveText('worker-10')

  const requestsBeforeRecovery = state.inventoryRequests
  state.runtimeInventory = {
    ...state.runtimeInventory,
    observed_at_unix_ms: Date.now(),
    workers: state.runtimeInventory.workers.map((candidate) =>
      candidate.runtime_id === 'terminal-10'
        ? { ...candidate, status: 'blocked' }
        : candidate,
    ),
  }
  state.inventoryFailure = false

  await expect
    .poll(() => state.inventoryRequests, { timeout: 3_000 })
    .toBeGreaterThan(requestsBeforeRecovery)
  await expect(
    page.getByText('Synthetic inventory snapshot failure'),
  ).toHaveCount(0)
  await expect(page.locator('.inspector .status-badge')).toHaveAttribute(
    'data-status',
    'blocked',
  )
})

test('reloads worker candidates after inventory reconciliation', async ({
  page,
}) => {
  await mockApi(page, { reconcileWorkerOnInventory: true })
  await page.setViewportSize({ width: 1200, height: 760 })
  await page.goto('/')

  await page.getByRole('tab', { name: 'Workers' }).click()
  const reconciledWorker = page.locator(
    '.worker-row[data-worker-id="worker-reconciled"]',
  )
  await expect(reconciledWorker).toBeVisible()
  await expect(reconciledWorker).toHaveAttribute('draggable', 'true')
  await expect(reconciledWorker).toContainText('Unassigned live')
})

test('shows selected worker details on mobile', async ({ page }, testInfo) => {
  await mockApi(page)
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/')

  await expect(page.locator('.project-region').first()).toBeVisible()
  await page.getByRole('tab', { name: 'Workers' }).click()
  await page.locator('.worker-row').first().click()
  await expect(page.locator('.inspector.has-selection')).toBeVisible()
  await expect(page.getByText('Worker candidate')).toBeVisible()

  const overflow = await page.evaluate(() => ({
    horizontal: document.documentElement.scrollWidth - window.innerWidth,
    vertical: document.documentElement.scrollHeight - window.innerHeight,
  }))
  expect(overflow.horizontal).toBeLessThanOrEqual(0)
  expect(overflow.vertical).toBeLessThanOrEqual(0)

  await page.screenshot({
    path: testInfo.outputPath('mobile.png'),
    fullPage: true,
  })
})

test('keeps profile creation available on mobile', async ({ page }) => {
  await mockApi(page)
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/')

  await expect(page.locator('.profile-row')).toHaveCount(1)
  await page.getByRole('button', { name: 'Create worker profile' }).click()

  await expect(page.getByRole('dialog', { name: 'New profile' })).toBeVisible()
})

test('opens workspace-backed project creation on mobile', async ({
  page,
}, testInfo) => {
  await mockApi(page)
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/')

  await page
    .getByRole('button', { name: 'Create project', exact: true })
    .click()

  const inspector = page.locator('.inspector.has-selection')
  await expect(inspector).toBeVisible()
  await expect(inspector.locator('#new-project-name')).toBeVisible()
  const overflow = await page.evaluate(() => ({
    horizontal: document.documentElement.scrollWidth - window.innerWidth,
    vertical: document.documentElement.scrollHeight - window.innerHeight,
  }))
  expect(overflow.horizontal).toBeLessThanOrEqual(0)
  expect(overflow.vertical).toBeLessThanOrEqual(0)
  await page.screenshot({
    path: testInfo.outputPath('workspace-project-mobile.png'),
    fullPage: true,
  })
})

test('keeps durable projects when a runtime snapshot fails', async ({ page }) => {
  await mockApi(page, { betaFails: true })
  await page.setViewportSize({ width: 1200, height: 760 })
  await page.goto('/')
  await expect(page.locator('.project-region')).toHaveCount(2)

  await selectRuntimeSession(page, 'beta')

  await expect(page.getByText('Synthetic beta snapshot failure')).toBeVisible()
  await expect(page.locator('.worker-marker')).toHaveCount(2)
  await expect(page.locator('.orchestrator-marker')).toHaveCount(2)
  await expect(page.locator('.project-region')).toHaveCount(2)
  await expect(
    page.locator('.project-region[data-runtime="offline"]'),
  ).toHaveCount(2)
})

test('creates a project workspace and orchestrator from the empty inspector', async ({
  page,
}, testInfo) => {
  const state = await mockApi(page)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  const shelf = page.locator('.resource-shelf')
  await expect(shelf).toBeVisible()
  await page
    .getByRole('button', { name: 'Collapse resource shelf' })
    .click()
  await expect(shelf).toHaveCount(0)

  const createProject = page.getByRole('button', {
    name: 'Create project',
    exact: true,
  })
  await expect(createProject).toBeVisible()
  await createProject.click()
  const inspector = page.locator('.inspector.has-selection')
  await expect(
    inspector.getByRole('heading', { name: 'Create project' }),
  ).toBeVisible()
  await inspector.getByLabel('Project name').fill('Workspace-backed project')
  await inspector.getByLabel('Checkout path').fill('/tmp/workspace-backed')
  await page
    .getByLabel('Orchestrator brief')
    .fill('Coordinate the workspace-backed project and report blockers.')
  await inspector
    .getByRole('button', { name: 'Create project', exact: true })
    .click()

  await expect(page.locator('.project-region')).toHaveCount(3)
  await expect(
    page.getByRole('heading', { name: 'Workspace-backed project' }),
  ).toBeVisible()
  expect(state.workspaceProjectCommands).toHaveLength(1)
  expect(state.workspaceProjectCommands[0]).toMatchObject({
    actor: 'local-user',
    name: 'Workspace-backed project',
    runtime_adapter: 'herdr',
    runtime_session: 'alpha',
    workspace_label: 'Workspace-backed project',
    cwd: '/tmp/workspace-backed',
    profile_id: state.profiles[0].id,
    expected_profile_version: state.profiles[0].version,
    orchestrator_objective:
      'Coordinate the workspace-backed project and report blockers.',
  })
  expect(state.projects[2].runtime.workspace_id).toBe(
    'workspace-created-1',
  )
  expect(state.projects[2].orchestrator.profile_id).toBe(
    state.profiles[0].id,
  )
  await page.screenshot({
    path: testInfo.outputPath('workspace-project-created.png'),
    fullPage: true,
  })

  await page.reload()
  await expect(page.locator('.project-region')).toHaveCount(3)
  expect(state.workspaceProjectCommands).toHaveLength(1)
})

test('adopts an observed workspace and keeps it after reload', async ({ page }) => {
  const state = await mockApi(page)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page.getByRole('tab', { name: 'Workspaces' }).click()
  await page.locator('.workspace-row').first().click()
  await expect(
    page.getByRole('paragraph').filter({ hasText: 'Create project' }),
  ).toBeVisible()
  await page.locator('#project-name').fill('Release project')
  await page
    .locator('form')
    .getByRole('button', { name: 'Create project' })
    .click()

  await expect(page.locator('.project-region')).toHaveCount(3)
  await expect(page.getByText('Yard project', { exact: true })).toBeVisible()
  expect(state.projects).toHaveLength(3)
  expect(state.projects[2].orchestrator.id).not.toBe(
    state.projects[2].orchestrator.runtime!.terminal_id,
  )

  await page.reload()
  await expect(page.locator('.project-region')).toHaveCount(3)
  await expect(page.getByText('Release project')).toBeVisible()
})

test('creates a project orchestrator from a selected profile', async ({
  page,
}) => {
  const state = await mockApi(page)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page.getByRole('tab', { name: 'Workspaces' }).click()
  await page.locator('.workspace-row').first().click()
  await page.getByRole('button', { name: 'Profile', exact: true }).click()
  await page.locator('#project-name').fill('Profile-backed project')
  await page
    .locator('#orchestrator-objective')
    .fill('Coordinate the release checks and report blockers.')
  await page
    .locator('form')
    .getByRole('button', { name: 'Create project' })
    .click()

  await expect(page.locator('.project-region')).toHaveCount(3)
  await expect(page.locator('.orchestrator-marker')).toHaveCount(3)
  expect(state.profileProjectCommands).toHaveLength(1)
  expect(state.profileProjectCommands[0]).toMatchObject({
    actor: 'local-user',
    name: 'Profile-backed project',
    profile_id: state.profiles[0].id,
    expected_profile_version: state.profiles[0].version,
    orchestrator_objective:
      'Coordinate the release checks and report blockers.',
  })
  expect(state.projects[2].orchestrator.profile_id).toBe(
    state.profiles[0].id,
  )
  expect(state.projects[2].orchestrator.id).not.toBe(
    state.projects[2].orchestrator.runtime!.terminal_id,
  )
})

test('confirms profile allocation before rendering a durable worker', async ({
  page,
}) => {
  const state = await mockApi(page)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await expect(page.locator('.profile-row')).toHaveCount(1)
  await dragToProject(page.locator('.profile-row').first(), page)
  await expect(
    page.getByRole('dialog', { name: 'Create worker' }),
  ).toBeVisible()
  await page.getByLabel('Objective').fill('Implement the assignment slice.')
  await page
    .getByRole('button', { name: 'Create worker', exact: true })
    .click()

  const allocationEdge = page.locator(
    '.react-flow__edge.allocation-edge',
  )
  await expect(allocationEdge).toHaveCount(1)
  await expect(allocationEdge).not.toHaveAttribute('tabindex', /.+/)
  await expect(allocationEdge.locator('path.react-flow__edge-path')).toHaveAttribute(
    'd',
    /.+/,
  )

  await expect(page.locator('.assigned-worker-marker')).toHaveCount(1)
  await expect(page.getByText('Assignment', { exact: true })).toBeVisible()
  expect(state.assignments).toHaveLength(1)
  expect(state.assignments[0].objective).toBe(
    'Implement the assignment slice.',
  )

  await page.reload()
  await expect(page.locator('.assigned-worker-marker')).toHaveCount(1)
})

test('hands an assigned worker to another project from the canvas', async ({
  page,
}) => {
  const state = await mockApi(page)
  const source = seedAssignedCandidateAssignment(state)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  const marker = page.locator(
    '.assigned-worker-marker[data-worker-id="worker-assigned"]',
  )
  await expect(marker).toHaveAttribute('draggable', 'true')
  await dragToProject(marker, page, 1)

  const dialog = page.getByRole('dialog', { name: 'Move assigned worker' })
  await expect(dialog).toBeVisible()
  await expect(
    dialog.getByLabel('Handoff route').getByText('API migration'),
  ).toBeVisible()
  await expect(
    dialog.getByLabel('Handoff route').getByText('Offline release'),
  ).toBeVisible()
  await expect(dialog.getByLabel('Target role')).toHaveValue('member')
  await dialog.getByLabel('Objective').fill('Continue the release migration.')
  await dialog
    .getByRole('button', { name: 'Confirm handoff', exact: true })
    .click()

  await expect(dialog).toHaveCount(0)
  await expect(
    page.locator(
      '.assigned-worker-marker[data-worker-id="worker-assigned"]',
    ),
  ).toHaveCount(1)
  expect(state.handoffCommands).toHaveLength(1)
  expect(state.handoffCommands[0]).toMatchObject({
    actor: 'local-user',
    worker_id: 'worker-assigned',
    expected_worker_version: source.worker.version,
    expected_source_project_version: '1',
    target_project_id: 'project-2',
    expected_target_project_version: '1',
    source_attempt_id: source.attempt.id,
    expected_source_assignment_version: source.version,
    expected_source_attempt_version: source.attempt.version,
    target_role: 'member',
    objective: 'Continue the release migration.',
    role: 'implementer',
    isolation_policy: 'project_workspace',
  })
  const handedOff = state.assignments.find(
    (assignment) => assignment.id === source.id,
  )
  const target = state.assignments.find(
    (assignment) =>
      assignment.project_id === 'project-2' &&
      assignment.lifecycle === 'active',
  )
  expect(handedOff?.lifecycle).toBe('handed_off')
  expect(target?.worker.id).toBe(source.worker.id)
  expect(target?.worker.runtime?.workspace_id).toBe('workspace-offline')
  expect(
    state.workerCandidates.find(
      (candidate) => candidate.worker.id === source.worker.id,
    ),
  ).toMatchObject({
    availability: 'assigned',
    project_id: 'project-2',
    assignment_id: target?.id,
  })
})

test('keeps the handoff confirmation reachable on a narrow viewport', async ({
  page,
}) => {
  const state = await mockApi(page)
  seedAssignedCandidateAssignment(state)
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/')

  // Dropped on the territory's own upright label. The old fixed 10px offset
  // assumed a full-width heading bar; in 2.5D the label is a compact plaque
  // pinned to the projected centroid, so the centre is the stable point.
  await dragToProject(
    page.locator(
      '.assigned-worker-marker[data-worker-id="worker-assigned"]',
    ),
    page,
    1,
  )

  const dialog = page.getByRole('dialog', { name: 'Move assigned worker' })
  await expect(dialog).toBeVisible()
  await expect(
    dialog.getByRole('button', { name: 'Confirm handoff', exact: true }),
  ).toBeVisible()
  const bounds = await dialog.boundingBox()
  expect(bounds).not.toBeNull()
  expect(bounds!.x).toBeGreaterThanOrEqual(0)
  expect(bounds!.y).toBeGreaterThanOrEqual(0)
  expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(390)
  expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(844)
})

test('replaces a target orchestrator through the keyboard handoff path', async ({
  page,
}) => {
  const state = await mockApi(page)
  const source = seedAssignedCandidateAssignment(state)
  const replacedOrchestratorId = state.projects[1].orchestrator.id
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page.getByRole('tab', { name: 'Workers' }).click()
  const worker = page.locator(
    '.worker-row[data-worker-id="worker-assigned"]',
  )
  await expect(worker).toHaveAttribute('draggable', 'true')
  await worker.press('Enter')
  await page
    .getByRole('button', { name: 'Hand off worker', exact: true })
    .click()

  const dialog = page.getByRole('dialog', { name: 'Move assigned worker' })
  await dialog.getByLabel('Target role').selectOption('orchestrator')
  await expect(
    dialog.getByText(`Replaces worker ${replacedOrchestratorId.slice(0, 12)}`),
  ).toBeVisible()
  await dialog.getByLabel('Objective').fill('Coordinate the target project.')
  await dialog
    .getByRole('button', { name: 'Confirm handoff', exact: true })
    .click()

  await expect(
    page.locator(
      '.orchestrator-marker[data-project-id="project-2"][data-worker-id="worker-assigned"]',
    ),
  ).toBeVisible()
  await expect(
    page.locator(
      '.assigned-worker-marker[data-worker-id="worker-assigned"]',
    ),
  ).toHaveCount(0)
  expect(state.handoffCommands[0]).toMatchObject({
    worker_id: source.worker.id,
    target_project_id: 'project-2',
    target_role: 'orchestrator',
    objective: 'Coordinate the target project.',
  })
  expect(state.projects[1].orchestrator.id).toBe(source.worker.id)
  expect(
    state.workerCandidates.find(
      (candidate) => candidate.worker.id === source.worker.id,
    )?.availability,
  ).toBe('orchestrator')
})

test('retains the handoff command ID while a proposal remains open', async ({
  page,
}) => {
  const state = await mockApi(page, { handoffFailsOnce: true })
  seedAssignedCandidateAssignment(state)
  await page.setViewportSize({ width: 1200, height: 760 })
  await page.goto('/')

  await page.getByRole('tab', { name: 'Workers' }).click()
  await page
    .locator('.worker-row[data-worker-id="worker-assigned"]')
    .press('Enter')
  await page
    .getByRole('button', { name: 'Hand off worker', exact: true })
    .click()
  const dialog = page.getByRole('dialog', { name: 'Move assigned worker' })
  const submit = dialog.getByRole('button', {
    name: 'Confirm handoff',
    exact: true,
  })
  await submit.click()

  await expect(dialog).toBeVisible()
  await expect(dialog.locator('.dialog-error')).toBeVisible()
  await expect(submit).toBeEnabled()
  await submit.click()

  await expect(dialog).toHaveCount(0)
  expect(state.handoffRequestCommandIds).toHaveLength(2)
  expect(state.handoffRequestCommandIds[0]).toBe(
    state.handoffRequestCommandIds[1],
  )
  expect(
    state.assignments.filter(
      (assignment) =>
        assignment.project_id === 'project-2' &&
        assignment.lifecycle === 'active',
    ),
  ).toHaveLength(1)
})

test('allocates a profileless live worker from the keyboard inspector action', async ({
  page,
}) => {
  const state = await mockApi(page)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await expect(page.locator('.profile-row')).toHaveAttribute(
    'draggable',
    'true',
  )
  await page.getByRole('tab', { name: 'Workers' }).click()
  const liveWorker = page.locator(
    '.worker-row[data-worker-id="worker-unassigned"]',
  )
  const resumableWorker = page.locator(
    '.worker-row[data-worker-id="worker-resumable"]',
  )
  await expect(liveWorker).toHaveAttribute('draggable', 'true')
  await expect(resumableWorker).toHaveAttribute('draggable', 'true')
  await expect(
    page.locator('.worker-row[data-worker-id="worker-assigned"]'),
  ).toHaveAttribute('draggable', 'true')
  await expect(
    page.locator('.worker-row[data-worker-id="project-1-orchestrator"]'),
  ).toHaveAttribute('draggable', 'false')
  await expect(
    page.locator('.worker-row[data-worker-id="worker-unavailable"]'),
  ).toHaveAttribute('draggable', 'false')
  await expect(
    page.locator('.worker-row[data-worker-id="worker-ambiguous"]'),
  ).toHaveAttribute('draggable', 'false')

  await liveWorker.press('Enter')
  await expect(page.getByText('Worker candidate')).toBeVisible()
  await page
    .getByRole('button', { name: 'Assign worker', exact: true })
    .click()

  const dialog = page.getByRole('dialog', { name: 'Assign worker' })
  await expect(dialog).toBeVisible()
  await expect(dialog.getByLabel('Worker profile')).toHaveValue('')
  await expect(dialog.getByLabel('Role')).toHaveValue('worker')
  await dialog.getByLabel('Objective').fill('Take over the live worker.')
  await dialog
    .getByRole('button', { name: 'Assign worker', exact: true })
    .click()

  await expect(page.locator('.assigned-worker-marker')).toHaveCount(1)
  expect(state.allocationCommands).toHaveLength(1)
  expect(state.allocationCommands[0]).toMatchObject({
    actor: 'local-user',
    worker_id: 'worker-unassigned',
    expected_worker_version: '1',
    expected_project_version: '1',
    objective: 'Take over the live worker.',
    role: 'worker',
    isolation_policy: 'project_workspace',
  })
  expect(state.allocationCommands[0]).not.toHaveProperty('profile_id')
  expect(state.allocationCommands[0].command_id).toBeTruthy()
})

test('resumes a profiled worker through the generalized drag payload', async ({
  page,
}) => {
  const state = await mockApi(page)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page.getByRole('tab', { name: 'Workers' }).click()
  await dragToProject(
    page.locator('.worker-row[data-worker-id="worker-resumable"]'),
    page,
  )

  const dialog = page.getByRole('dialog', { name: 'Resume worker' })
  await expect(dialog).toBeVisible()
  await expect(dialog.getByLabel('Worker profile')).toHaveCount(0)
  await expect(dialog.getByLabel('Role')).toHaveValue('implementer')
  await dialog.getByLabel('Objective').fill('Resume the verification work.')
  await dialog
    .getByRole('button', { name: 'Resume worker', exact: true })
    .click()

  expect(state.allocationCommands).toHaveLength(1)
  expect(state.allocationCommands[0]).toMatchObject({
    worker_id: 'worker-resumable',
    expected_worker_version: '1',
    expected_project_version: '1',
    objective: 'Resume the verification work.',
    role: 'implementer',
  })
  expect(state.allocationCommands[0]).not.toHaveProperty('profile_id')
  expect(state.workerCandidates).toContainEqual(
    expect.objectContaining({
      availability: 'assigned',
      assignment_id: 'assignment-1',
      project_id: 'project-1',
    }),
  )
})

test('retains the allocation command ID while a proposal remains open', async ({
  page,
}) => {
  const state = await mockApi(page, { allocationFailsOnce: true })
  await page.setViewportSize({ width: 1200, height: 760 })
  await page.goto('/')

  await dragToProject(page.locator('.profile-row').first(), page)
  const dialog = page.getByRole('dialog', { name: 'Create worker' })
  await dialog.getByLabel('Objective').fill('Retry the same proposal.')
  const submit = dialog.getByRole('button', {
    name: 'Create worker',
    exact: true,
  })
  await submit.click()

  await expect(dialog).toBeVisible()
  await expect(dialog.locator('.dialog-error')).toBeVisible()
  await expect(submit).toBeEnabled()
  await submit.click()

  await expect(page.locator('.assigned-worker-marker')).toHaveCount(1)
  expect(state.allocationRequestCommandIds).toHaveLength(2)
  expect(state.allocationRequestCommandIds[0]).toBe(
    state.allocationRequestCommandIds[1],
  )
})

test('loads recent assignment activity when the chat opens', async ({
  page,
}) => {
  const state = await mockApi(page, {
    terminalOutputDelayMs: 150,
    terminalOutputText: (readCount) =>
      Array.from(
        { length: 240 },
        (_, index) => `build ${readCount}: check ${index + 1} still running`,
      ).join('\n'),
    terminalOutputTruncated: true,
  })
  seedActiveAssignment(state)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page.locator('.assigned-worker-marker').click()
  await expect(
    page.getByRole('button', { name: 'Open terminal', exact: true }),
  ).toBeVisible()
  await page.waitForTimeout(100)
  expect(state.terminalOutputRequests).toHaveLength(0)
  expect(state.terminalSockets).toHaveLength(0)

  await page
    .getByRole('button', { name: 'Open chat', exact: true })
    .click()
  await expect(page.getByText('Loading recent agent activity')).toBeVisible()
  const conversation = page.getByLabel('Agent conversation')
  await expect(conversation).toContainText('still running')
  const initialText = await conversation.textContent()
  expect(
    state.terminalOutputRequests.every(
      (request) =>
        request.projectId === 'project-1' &&
        request.assignmentId === 'assignment-1' &&
        request.lines === '160',
    ),
  ).toBe(true)

  const refresh = page.getByRole('button', {
    name: 'Refresh agent activity',
  })
  const messages = page.locator('.chat-thread__messages')
  await expect
    .poll(() =>
      messages.evaluate(
        (element) => element.scrollHeight - element.clientHeight,
      ),
    )
    .toBeGreaterThan(100)
  await messages.evaluate((element) => {
    element.scrollTop = 0
    element.dispatchEvent(new Event('scroll'))
  })
  await refresh.click()
  await expect(refresh).toBeDisabled()
  await expect(conversation).not.toHaveText(initialText ?? '')
  await expect(refresh).toBeEnabled()
  await expect
    .poll(() => messages.evaluate((element) => element.scrollTop))
    .toBe(0)
  expect(state.terminalOutputRequests.length).toBeGreaterThanOrEqual(2)

  await page.getByRole('button', { name: 'Close chat' }).click()
  await page
    .getByRole('button', { name: 'Open chat', exact: true })
    .click()
  await expect
    .poll(() => messages.evaluate((element) => element.scrollTop))
    .toBeGreaterThan(100)

  const overflow = await page.evaluate(() => {
    const workspace = document.querySelector<HTMLElement>('.chat-workspace')
    return {
      documentHorizontal:
        document.documentElement.scrollWidth - window.innerWidth,
      documentVertical:
        document.documentElement.scrollHeight - window.innerHeight,
      workspaceHorizontal: workspace
        ? workspace.scrollWidth - workspace.clientWidth
        : Number.POSITIVE_INFINITY,
    }
  })
  expect(overflow.documentHorizontal).toBeLessThanOrEqual(0)
  expect(overflow.documentVertical).toBeLessThanOrEqual(0)
  expect(overflow.workspaceHorizontal).toBeLessThanOrEqual(0)
})

test('keeps one terminal output snapshot in one chat bubble', async ({
  page,
}) => {
  const state = await mockApi(page, {
    terminalOutputText: [
      'Inspected the runtime state.',
      'Ran the focused checks.',
      '```text\nfirst result\n\nsecond result\n```',
      'Ready for owner review.',
    ].join('\n\n'),
  })
  seedActiveAssignment(state)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page.locator('.assigned-worker-marker').click()
  await page
    .getByRole('button', { name: 'Open chat', exact: true })
    .click()

  const outputBubbles = page
    .getByLabel('Agent conversation')
    .locator('.chat-message[data-kind="agent"]')
  await expect(outputBubbles).toHaveCount(1)
  await expect(outputBubbles).toContainText(
    'Inspected the runtime state.',
  )
  await expect(outputBubbles).toContainText(
    'Ran the focused checks.',
  )
  await expect(outputBubbles).toContainText(
    'first result\n\nsecond result',
  )
  await expect(outputBubbles).toContainText(
    'Ready for owner review.',
  )
  await expect(outputBubbles).toContainText('Agent output · rev')
  const bubbleWidth = await outputBubbles.evaluate((element) => {
    const thread = element.parentElement
    if (!thread) return Number.POSITIVE_INFINITY
    const style = getComputedStyle(thread)
    const contentWidth =
      thread.clientWidth -
      Number.parseFloat(style.paddingLeft) -
      Number.parseFloat(style.paddingRight)
    return Math.abs(element.getBoundingClientRect().width - contentWidth)
  })
  expect(bubbleWidth).toBeLessThanOrEqual(1)
  expect(state.terminalOutputRequests).toHaveLength(1)
})

test('shows agent activity error and empty states in chat', async ({
  page,
}) => {
  const state = await mockApi(page, {
    terminalOutputFails: true,
    terminalOutputText: '',
  })
  seedActiveAssignment(state)
  await page.setViewportSize({ width: 1200, height: 760 })
  await page.goto('/')

  await page.locator('.assigned-worker-marker').click()
  await page
    .getByRole('button', { name: 'Open chat', exact: true })
    .click()
  const conversation = page.getByLabel('Agent conversation')
  const initialHeight = await conversation.evaluate(
    (element) => element.getBoundingClientRect().height,
  )
  await expect(page.getByText('Agent output unavailable')).toBeVisible()
  await expect(page.getByText('Synthetic terminal output failure')).toBeVisible()

  state.terminalOutputFailure = false
  await page
    .getByRole('button', { name: 'Refresh agent activity' })
    .click()
  await expect(page.getByText('No recent agent output.')).toBeVisible()
  await expect
    .poll(() =>
      conversation.evaluate((element) => element.getBoundingClientRect().height),
    )
    .toBe(initialHeight)
})

test('connects the assignment terminal and relays frames, input, resize, and release', async ({
  page,
}, testInfo) => {
  const state = await mockApi(page)
  seedActiveAssignment(state)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page.locator('.assigned-worker-marker').click()
  expect(state.terminalSockets).toHaveLength(0)
  const openTerminal = page.getByRole('button', {
    name: 'Open terminal',
    exact: true,
  })
  await openTerminal.click()
  const terminal = page.locator('.terminal-session')
  await expect(terminal).toHaveAttribute('data-state', 'connected')
  await expect.poll(() => state.terminalSockets.length).toBe(1)
  await expect(
    page.getByRole('dialog', { name: 'Implementer' }),
  ).toBeVisible()

  const url = new URL(state.terminalConnectionUrls[0])
  expect(url.protocol).toBe('ws:')
  expect(url.pathname).toBe(
    '/api/v1/projects/project-1/assignments/assignment-1/terminal',
  )
  const cols = Number(url.searchParams.get('cols'))
  const rows = Number(url.searchParams.get('rows'))
  expect(cols).toBeGreaterThan(0)
  expect(rows).toBeGreaterThan(0)
  const initialTerminalHeight = await terminal.evaluate(
    (element) => element.getBoundingClientRect().height,
  )
  const desktopOverflow = await page.evaluate(() => {
    const inspector = document.querySelector<HTMLElement>('.inspector')
    const terminal = document.querySelector<HTMLElement>('.terminal-session')
    return {
      documentHorizontal:
        document.documentElement.scrollWidth - window.innerWidth,
      inspectorHorizontal: inspector
        ? inspector.scrollWidth - inspector.clientWidth
        : Number.POSITIVE_INFINITY,
      terminalHorizontal: terminal
        ? terminal.scrollWidth - terminal.clientWidth
        : Number.POSITIVE_INFINITY,
      terminalRight:
        terminal?.getBoundingClientRect().right ?? Number.POSITIVE_INFINITY,
      windowWidth: window.innerWidth,
    }
  })
  expect(desktopOverflow.documentHorizontal).toBeLessThanOrEqual(0)
  expect(desktopOverflow.inspectorHorizontal).toBeLessThanOrEqual(0)
  expect(desktopOverflow.terminalHorizontal).toBeLessThanOrEqual(0)
  expect(desktopOverflow.terminalRight).toBeLessThanOrEqual(
    desktopOverflow.windowWidth,
  )

  await expect
    .poll(() =>
      state.terminalMessages.some(
        (message) => message.type === 'terminal.resize',
      ),
    )
    .toBe(true)
  expect(
    state.terminalMessages.find(
      (message) => message.type === 'terminal.resize',
    ),
  ).toEqual({
    type: 'terminal.resize',
    cols,
    rows,
  })

  state.terminalSockets[0].send(
    JSON.stringify({
      type: 'terminal.frame',
      bytes: Buffer.from(
        `${Array.from(
          { length: 140 },
          (_, index) => `history line ${index + 1}`,
        ).join('\r\n')}\r\n\u001b[32mterminal ready\u001b[0m`,
      ).toString('base64'),
      seq: 7,
      width: cols,
      height: rows,
      full: true,
    }),
  )
  await expect(terminal).toHaveAttribute('data-frame-sequence', '7')
  await expect(terminal.locator('.xterm-screen')).toContainText(
    'terminal ready',
  )
  const terminalRows = terminal.locator('.xterm-accessibility-tree')
  await expect(terminalRows).toContainText('terminal ready')
  await terminal.locator('.xterm-scrollable-element').hover()
  await page.mouse.wheel(0, -1200)
  await expect(terminalRows).not.toContainText('terminal ready')
  await expect(terminalRows).toContainText('history line')
  state.terminalSockets[0].send(
    JSON.stringify({
      type: 'terminal.frame',
      bytes: Buffer.from('\r\nnew output while reviewing history').toString(
        'base64',
      ),
      seq: 8,
      width: cols,
      height: rows,
      full: false,
    }),
  )
  await expect(terminal).toHaveAttribute('data-frame-sequence', '8')
  await expect(terminalRows).not.toContainText(
    'new output while reviewing history',
  )
  await expect(terminalRows).toContainText('history line')

  state.terminalSockets[0].send(
    JSON.stringify({
      type: 'terminal.frame',
      bytes: Buffer.from('\u001b[?1049hfull-screen terminal app').toString(
        'base64',
      ),
      seq: 9,
      width: cols,
      height: rows,
      full: false,
    }),
  )
  await expect(terminalRows).toContainText('full-screen terminal app')
  const inputCountBeforeWheel = state.terminalMessages.filter(
    (message) => message.type === 'terminal.input',
  ).length
  await terminal.locator('.xterm-screen').hover()
  await page.mouse.wheel(0, -240)
  await expect
    .poll(
      () =>
        state.terminalMessages
          .filter((message) => message.type === 'terminal.input')
          .slice(inputCountBeforeWheel)
          .map((message) => message.text),
    )
    .toContain('\u001b[A')
  await page.screenshot({
    path: testInfo.outputPath('terminal-workspace-desktop.png'),
    fullPage: true,
  })
  await expect
    .poll(() =>
      terminal.evaluate((element) => element.getBoundingClientRect().height),
    )
    .toBe(initialTerminalHeight)

  await terminal.locator('.xterm-helper-textarea').pressSequentially('pwd')
  await expect
    .poll(() =>
      state.terminalMessages
        .filter((message) => message.type === 'terminal.input')
        .map((message) => message.text)
        .join(''),
    )
    .toContain('pwd')

  const initialResizeCount = state.terminalMessages.filter(
    (message) => message.type === 'terminal.resize',
  ).length
  await page.setViewportSize({ width: 390, height: 844 })
  await expect
    .poll(
      () =>
        state.terminalMessages.filter(
          (message) => message.type === 'terminal.resize',
        ).length,
    )
    .toBeGreaterThan(initialResizeCount)

  await page.getByRole('button', { name: 'Close terminal' }).click()
  await expect
    .poll(() =>
      state.terminalMessages.some(
        (message) => message.type === 'terminal.release',
      ),
    )
    .toBe(true)
  await expect(openTerminal).toBeFocused()
})

test('keeps a worker terminal theme and scrollback authoritative under TUI mouse mode', async ({
  page,
}, testInfo) => {
  await page.addInitScript(() => {
    window.localStorage.setItem('yard:theme', 'light')
  })
  const state = await mockApi(page)
  seedActiveAssignment(state)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page.locator('.assigned-worker-marker').click()
  await page
    .getByRole('button', { name: 'Open terminal', exact: true })
    .click()

  const workerDialog = page.getByRole('dialog', { name: 'Implementer' })
  const terminal = workerDialog.locator('.terminal-session')
  const xtermViewport = terminal.locator('.xterm-scrollable-element')
  const renderedRows = terminal.locator('.xterm-rows')
  const firstVisibleRow = terminal
    .locator('.xterm-accessibility-tree')
    .getByRole('listitem')
    .first()
  const xtermBackground = () =>
    xtermViewport.evaluate((element) => element.style.backgroundColor)
  const viewportY = () =>
    firstVisibleRow
      .getAttribute('aria-posinset')
      .then((position) => Number(position) - 1)

  await expect(terminal).toHaveAttribute('data-state', 'connected')
  await expect.poll(() => state.terminalSockets.length).toBe(1)
  expect(new URL(state.terminalConnectionUrls[0]).pathname).toBe(
    '/api/v1/projects/project-1/assignments/assignment-1/terminal',
  )
  await expect.poll(xtermBackground).toBe('rgb(237, 241, 239)')

  await setAppTheme(page, 'Dark')
  await expect.poll(xtermBackground).toBe('rgb(23, 32, 29)')
  await page.getByLabel('Terminal color theme').selectOption('nord')
  await expect.poll(xtermBackground).toBe('rgb(46, 52, 64)')

  const terminalUrl = new URL(state.terminalConnectionUrls[0])
  const cols = Number(terminalUrl.searchParams.get('cols'))
  const rows = Number(terminalUrl.searchParams.get('rows'))
  state.terminalSockets[0].send(
    JSON.stringify({
      type: 'terminal.frame',
      bytes: Buffer.from(
        [
          '\u001b]11;#ff0000\u0007',
          '\u001b[?1000h',
          ...Array.from(
            { length: 180 },
            (_, index) => `worker history line ${index + 1}\r\n`,
          ),
          'worker terminal tail',
        ].join(''),
      ).toString('base64'),
      seq: 30,
      width: cols,
      height: rows,
      full: true,
    }),
  )
  await expect(terminal).toHaveAttribute('data-frame-sequence', '30')
  await expect(
    terminal.locator('.xterm-accessibility-tree'),
  ).toContainText('worker terminal tail')
  await expect(renderedRows).toContainText('worker terminal tail')
  // Worker TUIs may set OSC colors after xterm is constructed. The selected
  // Yard palette remains authoritative for the renderer itself.
  await expect.poll(xtermBackground).toBe('rgb(46, 52, 64)')

  for (let sequence = 31; sequence <= 50; sequence += 1) {
    state.terminalSockets[0].send(
      JSON.stringify({
        type: 'terminal.frame',
        bytes: Buffer.from('\u001b]11;#ff0000\u0007').toString('base64'),
        seq: sequence,
        width: cols,
        height: rows,
        full: false,
      }),
    )
  }
  await expect(terminal).toHaveAttribute('data-frame-sequence', '50')
  await expect.poll(xtermBackground).toBe('rgb(46, 52, 64)')

  const bottomViewportY = await viewportY()
  expect(bottomViewportY).toBeGreaterThan(0)
  const inputCountBeforeWheel = state.terminalMessages.filter(
    (message) => message.type === 'terminal.input',
  ).length
  await terminal.locator('.xterm-screen').hover()
  await page.mouse.wheel(0, -1200)
  await expect.poll(viewportY).toBeLessThan(bottomViewportY)
  await expect(
    terminal.locator('.xterm-accessibility-tree'),
  ).toContainText('worker history line')
  await expect(renderedRows).not.toContainText('worker terminal tail')
  await expect(renderedRows).toContainText('worker history line')
  expect(
    state.terminalMessages.filter(
      (message) => message.type === 'terminal.input',
    ),
  ).toHaveLength(inputCountBeforeWheel)
  await page.screenshot({
    path: testInfo.outputPath('worker-terminal-theme-scrollback.png'),
    fullPage: true,
  })

  await terminal.locator('.xterm-helper-textarea').pressSequentially('pwd')
  await expect
    .poll(() =>
      state.terminalMessages
        .filter((message) => message.type === 'terminal.input')
        .map((message) => message.text)
        .join(''),
    )
    .toContain('pwd')
})

test('keeps one terminal lease and viewport across Terminal and Focus presentations', async ({
  page,
}, testInfo) => {
  const state = await mockApi(page)
  seedActiveAssignment(state)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page.locator('.assigned-worker-marker').click()
  await page
    .getByRole('button', { name: 'Open terminal', exact: true })
    .click()

  const shell = page.locator('.agent-workspace-shell')
  const terminal = page.locator('.terminal-session')
  const scrollable = terminal.locator('.xterm-scrollable-element')
  const terminalRows = terminal.locator('.xterm-accessibility-tree')
  const presentationControl = shell.getByRole('group', {
    name: 'Terminal presentation',
  })
  const terminalPresentation = presentationControl.getByRole('button', {
    name: 'Terminal',
    exact: true,
  })
  const focusPresentation = presentationControl.getByRole('button', {
    name: 'Focus',
    exact: true,
  })
  await expect(terminal).toHaveAttribute('data-state', 'connected')
  await expect.poll(() => state.terminalSockets.length).toBe(1)

  const terminalUrl = new URL(state.terminalConnectionUrls[0])
  const cols = Number(terminalUrl.searchParams.get('cols'))
  const rows = Number(terminalUrl.searchParams.get('rows'))
  state.terminalSockets[0].send(
    JSON.stringify({
      type: 'terminal.frame',
      bytes: Buffer.from(
        `${Array.from(
          { length: 180 },
          (_, index) => `focus history line ${index + 1}`,
        ).join('\r\n')}\r\nfocus terminal tail`,
      ).toString('base64'),
      seq: 20,
      width: cols,
      height: rows,
      full: true,
    }),
  )
  await expect(terminal).toHaveAttribute('data-frame-sequence', '20')
  await expect(terminalRows).toContainText('focus terminal tail')

  await scrollable.hover()
  await page.mouse.wheel(0, -1400)
  await expect(terminalRows).not.toContainText('focus terminal tail')
  await expect(terminalRows).toContainText('focus history line')
  const firstVisibleRow = terminalRows.getByRole('listitem').first()
  const terminalViewportPosition = Number(
    await firstVisibleRow.getAttribute('aria-posinset'),
  )
  expect(terminalViewportPosition).toBeGreaterThan(1)
  await terminal.evaluate((element) => {
    element.dataset.lifecycleMarker = 'same-terminal'
  })
  const terminalWidth = await terminal.evaluate(
    (element) => element.getBoundingClientRect().width,
  )

  await focusPresentation.click()
  await expect(shell).toHaveAttribute('data-presentation', 'focus')
  await expect(shell.getByLabel('Herdr windows')).toBeHidden()
  await expect(terminal).toHaveAttribute(
    'data-lifecycle-marker',
    'same-terminal',
  )
  await expect.poll(() => state.terminalSockets.length).toBe(1)
  expect(state.terminalConnectionUrls).toHaveLength(1)
  expect(
    state.terminalMessages.filter(
      (message) => message.type === 'terminal.release',
    ),
  ).toHaveLength(0)
  await expect
    .poll(() =>
      firstVisibleRow.getAttribute('aria-posinset').then(Number),
    )
    .toBe(terminalViewportPosition)
  await expect(terminalRows).not.toContainText('focus terminal tail')
  await expect(terminalRows).toContainText('focus history line')
  await expect
    .poll(() =>
      terminal.evaluate((element) => element.getBoundingClientRect().width),
    )
    .toBeGreaterThan(terminalWidth)
  await page.screenshot({
    path: testInfo.outputPath('terminal-focus-desktop.png'),
    fullPage: true,
  })

  await terminalPresentation.click()
  await expect(shell).toHaveAttribute('data-presentation', 'terminal')
  await expect(shell.getByLabel('Herdr windows')).toBeVisible()
  await expect(terminal).toHaveAttribute(
    'data-lifecycle-marker',
    'same-terminal',
  )
  await expect.poll(() => state.terminalSockets.length).toBe(1)
  expect(
    state.terminalMessages.filter(
      (message) => message.type === 'terminal.release',
    ),
  ).toHaveLength(0)
  await expect
    .poll(() =>
      firstVisibleRow.getAttribute('aria-posinset').then(Number),
    )
    .toBe(terminalViewportPosition)

  await page.setViewportSize({ width: 390, height: 844 })
  await expect(presentationControl).toBeVisible()
  await focusPresentation.click()
  await expect(shell).toHaveAttribute('data-presentation', 'focus')
  await expect.poll(() => state.terminalSockets.length).toBe(1)
  await expect(terminalRows).not.toContainText('focus terminal tail')
  await expect(terminalRows).toContainText('focus history line')
  const mobileLayout = await page.evaluate(() => {
    const shellElement =
      document.querySelector<HTMLElement>('.agent-workspace-shell')
    const terminalElement =
      document.querySelector<HTMLElement>('.terminal-session')
    return {
      documentHorizontal:
        document.documentElement.scrollWidth - window.innerWidth,
      shellRight:
        shellElement?.getBoundingClientRect().right ??
        Number.POSITIVE_INFINITY,
      terminalRight:
        terminalElement?.getBoundingClientRect().right ??
        Number.POSITIVE_INFINITY,
    }
  })
  expect(mobileLayout.documentHorizontal).toBeLessThanOrEqual(0)
  expect(mobileLayout.shellRight).toBeLessThanOrEqual(390)
  expect(mobileLayout.terminalRight).toBeLessThanOrEqual(390)
  await page.screenshot({
    path: testInfo.outputPath('terminal-focus-mobile.png'),
    fullPage: true,
  })

  await shell.getByRole('button', { name: 'Close terminal' }).click()
  await expect
    .poll(
      () =>
        state.terminalMessages.filter(
          (message) => message.type === 'terminal.release',
        ).length,
    )
    .toBe(1)
})

test('keeps the full-screen terminal surfaces synchronized with the app theme', async ({
  page,
}, testInfo) => {
  await page.addInitScript(() => {
    window.localStorage.setItem('yard:theme', 'light')
  })
  const state = await mockApi(page)
  seedActiveAssignment(state)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page.locator('.assigned-worker-marker').click()
  await page
    .getByRole('button', { name: 'Open terminal', exact: true })
    .click()

  const terminal = page.locator('.terminal-session')
  const viewport = terminal.locator('.xterm-viewport')
  await expect(terminal).toHaveAttribute('data-terminal-theme', 'light')
  await expect(viewport).toHaveCSS(
    'background-color',
    'rgb(237, 241, 239)',
  )
  await page.screenshot({
    path: testInfo.outputPath('terminal-theme-light.png'),
    fullPage: true,
  })

  await setAppTheme(page, 'Dark')
  await expect(terminal).toHaveAttribute('data-terminal-theme', 'dark')
  await expect(viewport).toHaveCSS(
    'background-color',
    'rgb(23, 32, 29)',
  )
  await expect(page.locator('.agent-workspace-shell')).toHaveCSS(
    'background-color',
    'rgb(23, 32, 29)',
  )
  await page.screenshot({
    path: testInfo.outputPath('terminal-theme-dark.png'),
    fullPage: true,
  })
})

test('lets the terminal use its own named color palette, independent of the app theme, and persists the choice', async ({
  page,
}) => {
  const state = await mockApi(page)
  seedActiveAssignment(state)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page.locator('.assigned-worker-marker').click()
  await page
    .getByRole('button', { name: 'Open terminal', exact: true })
    .click()

  const terminal = page.locator('.terminal-session')
  const viewport = terminal.locator('.xterm-viewport')
  const paletteSelect = page.getByLabel('Terminal color theme')

  await expect(paletteSelect).toHaveValue('auto')
  await expect(terminal).toHaveAttribute('data-terminal-palette', 'auto')

  await paletteSelect.selectOption('nord')
  await expect(terminal).toHaveAttribute('data-terminal-palette', 'nord')
  await expect(viewport).toHaveCSS('background-color', 'rgb(46, 52, 64)')

  for (const palette of [
    'nord',
    'dracula',
    'solarized-dark',
    'solarized-light',
    'gruvbox-dark',
  ]) {
    await paletteSelect.selectOption(palette)
    await expect(terminal).toHaveAttribute('data-terminal-palette', palette)
    const chrome = await terminal.evaluate((element) => {
      const terminalStyle = getComputedStyle(element)
      const status = element.querySelector<HTMLElement>(
        '.terminal-session__status',
      )
      const paletteControl = element.querySelector<HTMLElement>(
        '.terminal-session__palette-select',
      )
      const channels = (color: string) =>
        [...color.matchAll(/\d+(?:\.\d+)?/g)]
          .slice(0, 3)
          .map((match) => Number(match[0]) / 255)
          .map((channel) =>
            channel <= 0.04045
              ? channel / 12.92
              : ((channel + 0.055) / 1.055) ** 2.4,
          )
      const luminance = (color: string) => {
        const [red, green, blue] = channels(color)
        return 0.2126 * red + 0.7152 * green + 0.0722 * blue
      }
      const contrast = (foreground: string, background: string) => {
        const foregroundLuminance = luminance(foreground)
        const backgroundLuminance = luminance(background)
        return (
          (Math.max(foregroundLuminance, backgroundLuminance) + 0.05) /
          (Math.min(foregroundLuminance, backgroundLuminance) + 0.05)
        )
      }
      const background = terminalStyle.backgroundColor
      const border = paletteControl
        ? getComputedStyle(paletteControl).borderTopColor
        : ''
      const statusColor = status ? getComputedStyle(status).color : ''
      return {
        background,
        border,
        borderContrast: contrast(border, background),
        status: statusColor,
        statusContrast: contrast(statusColor, background),
      }
    })
    expect(chrome.border).not.toBe(chrome.background)
    expect(chrome.status).not.toBe(chrome.background)
    expect(chrome.borderContrast).toBeGreaterThanOrEqual(3)
    expect(chrome.statusContrast).toBeGreaterThanOrEqual(4.5)
  }
  await paletteSelect.selectOption('nord')
  await expect(terminal).toHaveAttribute('data-terminal-palette', 'nord')

  // Toggling the app-wide light/dark theme must not change the terminal's
  // colors while a named palette is selected.
  await setAppTheme(page, 'Dark')
  await expect(viewport).toHaveCSS('background-color', 'rgb(46, 52, 64)')
  await expect(terminal).toHaveAttribute('data-terminal-palette', 'nord')

  // The choice survives a reload.
  await page.reload()
  await page.locator('.assigned-worker-marker').click()
  await page
    .getByRole('button', { name: 'Open terminal', exact: true })
    .click()

  const reloadedTerminal = page.locator('.terminal-session')
  const reloadedViewport = reloadedTerminal.locator('.xterm-viewport')
  await expect(page.getByLabel('Terminal color theme')).toHaveValue('nord')
  await expect(reloadedTerminal).toHaveAttribute(
    'data-terminal-palette',
    'nord',
  )
  await expect(reloadedViewport).toHaveCSS(
    'background-color',
    'rgb(46, 52, 64)',
  )

  // Switching back to Auto restores the app-theme-derived palette.
  await page.getByLabel('Terminal color theme').selectOption('auto')
  await expect(reloadedTerminal).toHaveAttribute(
    'data-terminal-palette',
    'auto',
  )
  await expect(reloadedViewport).toHaveCSS(
    'background-color',
    'rgb(23, 32, 29)',
  )

  await page.evaluate(() => {
    window.localStorage.setItem('yard:terminal-palette', 'not-a-palette')
  })
  await page.reload()
  await page.locator('.assigned-worker-marker').click()
  await page
    .getByRole('button', { name: 'Open terminal', exact: true })
    .click()
  await expect(page.getByLabel('Terminal color theme')).toHaveValue('auto')
})

test('shows terminal closure and reopens only when requested', async ({ page }) => {
  const state = await mockApi(page)
  seedActiveAssignment(state)
  await page.setViewportSize({ width: 1200, height: 760 })
  await page.goto('/')

  await page.locator('.assigned-worker-marker').click()
  await page
    .getByRole('button', { name: 'Open terminal', exact: true })
    .click()
  const terminal = page.locator('.terminal-session')
  await expect(terminal).toHaveAttribute('data-state', 'connected')
  await expect.poll(() => state.terminalSockets.length).toBe(1)

  state.terminalSockets[0].send(
    JSON.stringify({
      type: 'terminal.closed',
      code: 'terminal_ownership_unavailable',
      reason: 'Terminal ownership unavailable',
    }),
  )
  await expect(terminal).toHaveAttribute('data-state', 'closed')
  await expect(
    terminal.getByText('Terminal ownership unavailable'),
  ).toBeVisible()

  await page.waitForTimeout(400)
  expect(state.terminalConnectionUrls).toHaveLength(1)

  await page.getByRole('button', { name: 'Reopen terminal' }).click()
  await expect(terminal).toHaveAttribute('data-state', 'connected')
  await expect.poll(() => state.terminalSockets.length).toBe(2)
  expect(state.terminalConnectionUrls).toHaveLength(2)
})

test('reconnects the Superintendent terminal when its runtime identity changes', async ({
  page,
}) => {
  const state = await mockApi(page)
  const originalWorker = durableWorker(
    'worker-yard-orchestrator',
    'terminal-yard-orchestrator',
    state.profiles[0],
    'workspace-yard-orchestrator',
    'yard-orchestrator',
  )
  state.yardOrchestrator = {
    worker: originalWorker,
    version: '2',
    workflow_profile_version: '1',
    created_at_unix_ms: Date.now(),
    updated_at_unix_ms: Date.now(),
  }
  state.runtimeSessions.push({
    name: 'yard-orchestrator',
    is_default: false,
    running: true,
  })
  await page.setViewportSize({ width: 1200, height: 760 })
  await page.goto('/')

  await page.locator('.yard-orchestrator-marker').click()
  await page
    .getByRole('button', { name: 'Open terminal', exact: true })
    .click()
  const terminal = page.locator('.terminal-session')
  await expect(terminal).toHaveAttribute('data-state', 'connected')
  await expect.poll(() => state.terminalSockets.length).toBe(1)
  expect(new URL(state.terminalConnectionUrls[0]).pathname).toBe(
    '/api/v1/yard/orchestrator/terminal',
  )

  const replacementWorker = durableWorker(
    'worker-yard-orchestrator-replacement',
    'terminal-yard-orchestrator-replacement',
    state.profiles[0],
    'workspace-yard-orchestrator-replacement',
    'yard-orchestrator',
  )
  state.yardOrchestrator = {
    worker: replacementWorker,
    version: '3',
    workflow_profile_version: '1',
    created_at_unix_ms: state.yardOrchestrator.created_at_unix_ms,
    updated_at_unix_ms: Date.now(),
  }

  await expect
    .poll(
      () =>
        state.terminalMessages.filter(
          (message) => message.type === 'terminal.release',
        ).length,
      { timeout: 5_000 },
    )
    .toBe(1)
  await expect.poll(() => state.terminalSockets.length).toBe(2)
  await expect(terminal).toHaveAttribute('data-state', 'connected')
  expect(new URL(state.terminalConnectionUrls[1]).pathname).toBe(
    '/api/v1/yard/orchestrator/terminal',
  )
})

test('provisions a dedicated Superintendent and shows project updates', async ({
  page,
}, testInfo) => {
  const state = await mockApi(page)
  const active = assignment(
    'assignment-portfolio-active',
    'project-1',
    state.profiles[0],
    'Finish the API compatibility migration',
    'implementer',
  )
  const completed = assignment(
    'assignment-portfolio-complete',
    'project-2',
    state.profiles[0],
    'Publish the offline release',
    'release owner',
  )
  completed.lifecycle = 'completed'
  completed.attempt.lifecycle = 'completed'
  completed.completion_receipt = {
    id: 'receipt-portfolio-complete',
    assignment_id: completed.id,
    attempt_id: completed.attempt.id,
    outcome: 'completed',
    summary: 'Published the signed offline release bundle.',
    artifact_refs: [],
    artifacts: [],
    evidence_refs: ['test://offline-release'],
    unresolved_blockers: [],
    actor: 'local-user',
    created_at_unix_ms: Date.now(),
  }
  state.assignments = [active, completed]
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  const hub = page.locator('.yard-orchestrator-marker')
  await expect(hub).toHaveCount(1)
  await expect(hub).toHaveAttribute('data-configured', 'false')
  const hubBounds = await hub.boundingBox()
  expect(hubBounds?.x).toBeGreaterThanOrEqual(0)
  expect(hubBounds?.y).toBeGreaterThanOrEqual(0)
  expect(hubBounds ? hubBounds.x + hubBounds.width : Infinity).toBeLessThanOrEqual(
    1280,
  )
  await hub.click()
  await expect(
    page.getByRole('heading', { name: 'Superintendent' }),
  ).toBeVisible()
  await expect(page.getByLabel('Orchestrator profile')).toHaveValue('profile-1')
  await page
    .getByRole('button', { name: 'Start Superintendent', exact: true })
    .click()

  await expect(hub).toHaveAttribute('data-configured', 'true')
  await expect(hub).toHaveAttribute(
    'data-worker-id',
    'worker-yard-orchestrator',
  )
  expect(state.yardOrchestratorProvisionCommands).toHaveLength(1)
  expect(state.yardOrchestratorProvisionCommands[0]).toMatchObject({
    profile_id: 'profile-1',
    expected_profile_version: '1',
    expected_orchestrator_version: '1',
  })
  await expect(page.getByText('Dedicated Herdr session')).toBeVisible()
  await page.getByRole('button', { name: 'Project pulse' }).click()
  await expect(page.getByLabel('Latest project updates')).toContainText(
    'Finish the API compatibility migration',
  )
  await expect(page.getByLabel('Latest project updates')).toContainText(
    'Published the signed offline release bundle.',
  )
  await page.getByRole('button', { name: 'Close Project pulse' }).click()

  await page
    .getByRole('button', { name: 'Open terminal', exact: true })
    .click()
  await expect(page.locator('.terminal-session')).toHaveAttribute(
    'data-state',
    'connected',
  )
  expect(new URL(state.terminalConnectionUrls[0]).pathname).toBe(
    '/api/v1/yard/orchestrator/terminal',
  )
  await page.getByRole('button', { name: 'Close terminal' }).click()

  await page
    .getByRole('button', { name: 'Open chat', exact: true })
    .click()
  await expect(page.getByLabel('Agent conversation')).toContainText(
    'Reviewing attention across the Yard portfolio.',
  )
  expect(state.yardOrchestratorOutputRequests).toEqual(['160'])

  await page
    .getByLabel('Message', { exact: true })
    .fill('Review every project and prioritize the blocked work.')
  await page
    .getByRole('button', { name: 'Send order', exact: true })
    .click()
  await expect(
    page.getByText('Order delivered to the agent.'),
  ).toBeVisible()
  expect(state.yardOrchestratorPromptCommands[0]).toMatchObject({
    actor: 'local-user',
    expected_orchestrator_version: '2',
    orchestrator_worker_id: 'worker-yard-orchestrator',
  })
  expect(state.yardOrchestratorPromptCommands[0].text).toContain(
    'Review every project and prioritize the blocked work.',
  )
  expect(state.yardOrchestratorPromptCommands[0].text).toContain(
    'MANUAL INTERVENTION',
  )
  expect(state.yardOrchestratorPromptDeliveries).toHaveLength(1)

  await page
    .getByLabel('Dispatch scope')
    .selectOption('project-1')
  await page
    .getByLabel('Message', { exact: true })
    .fill('Pick up the highest-priority blocked work.')
  await page
    .getByRole('button', { name: 'Send order', exact: true })
    .click()
  await expect(
    page.getByText('Yard delivered this order to API migration.'),
  ).toBeVisible()
  expect(state.yardOrchestratorRouteCommands).toHaveLength(1)
  expect(state.yardOrchestratorRouteCommands[0]).toMatchObject({
    orchestrator_worker_id: 'worker-yard-orchestrator',
    target_project_id: 'project-1',
    target_orchestrator_worker_id: 'project-1-orchestrator',
  })
  await expect(page.locator('.coordination-edge')).toHaveCount(
    state.projects.length,
  )
  await expect(
    page.locator('.coordination-edge.coordination-edge--active'),
  ).toHaveCount(1)

  await page.getByRole('button', { name: 'Close chat' }).click()
  await page.screenshot({
    path: testInfo.outputPath('yard-orchestrator-desktop.png'),
    fullPage: true,
  })
  await page.getByRole('button', { name: 'Project pulse' }).click()
  await page
    .getByRole('button', { name: /API migration.*In progress/ })
    .click()
  await expect(
    page.getByText('Project orchestrator', { exact: true }),
  ).toBeVisible()
  await expect(
    page.getByRole('heading', { name: 'API migration' }),
  ).toBeVisible()
})

test('recovers a stopped dedicated Superintendent session without replacing it', async ({
  page,
}) => {
  const state = await mockApi(page)
  const configuredWorker = durableWorker(
    'worker-yard-orchestrator',
    'terminal-yard-orchestrator',
    state.profiles[0],
    'workspace-yard-orchestrator',
    'yard-orchestrator',
  )
  state.yardOrchestrator = {
    worker: configuredWorker,
    version: '3',
    workflow_profile_version: '1',
    created_at_unix_ms: Date.now(),
    updated_at_unix_ms: Date.now(),
  }
  state.runtimeSessions.push({
    name: 'yard-orchestrator',
    is_default: false,
    running: false,
  })
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page.locator('.yard-orchestrator-marker').click()
  await expect(page.getByText('Dedicated session stopped')).toBeVisible()
  await expect(
    page.getByRole('button', { name: 'Open chat', exact: true }),
  ).toHaveCount(0)
  await page
    .getByRole('button', {
      name: 'Restart orchestrator session',
      exact: true,
    })
    .click()

  expect(state.yardOrchestratorRecoveryCommands).toHaveLength(1)
  expect(state.yardOrchestratorRecoveryCommands[0]).toMatchObject({
    actor: 'local-user',
    expected_orchestrator_version: '3',
  })
  await expect(page.getByText('Dedicated Herdr session')).toBeVisible()
  await expect(
    page.getByRole('button', { name: 'Open chat', exact: true }),
  ).toBeVisible()
  expect(state.yardOrchestrator.worker?.id).toBe(
    'worker-yard-orchestrator',
  )
})

test('uses full-screen chat and terminal modes with a Herdr window navigator', async ({
  page,
}, testInfo) => {
  const state = await mockApi(page)
  const implementer = seedActiveAssignment(state)
  const reviewerProfile = profile(
    'profile-reviewer',
    'Implementer',
  )
  const reviewer = assignment(
    'assignment-reviewer',
    'project-1',
    reviewerProfile,
    'Review release readiness.',
    'reviewer',
    durableWorker(
      'worker-reviewer',
      'terminal-reviewer',
      reviewerProfile,
      'workspace-2',
    ),
  )
  state.profiles.push(reviewerProfile)
  state.assignments.push(reviewer)
  state.runtimeSessions.push({
    name: 'gamma',
    is_default: false,
    running: false,
  })

  const apiWorkspace = state.runtimeInventory.workspaces.find(
    (candidate) => candidate.runtime_id === 'workspace-1',
  )
  const releaseWorkspace = state.runtimeInventory.workspaces.find(
    (candidate) => candidate.runtime_id === 'workspace-2',
  )
  if (!apiWorkspace || !releaseWorkspace) {
    throw new Error('Workspace navigator fixture is incomplete')
  }
  apiWorkspace.order = 20
  apiWorkspace.worktree = {
    repository_key: 'yard',
    repository_name: 'yard',
    repository_root: '/tmp/sample/yard',
    checkout_path: '/tmp/sample/yard-worktrees/api-migration',
    is_linked: true,
  }
  releaseWorkspace.order = 10
  releaseWorkspace.worktree = {
    repository_key: 'release-tools',
    repository_name: 'release-tools',
    repository_root: '/tmp/sample/release-tools',
    checkout_path: '/tmp/sample/release-tools/checks',
    is_linked: true,
  }

  const implementerRuntime = implementer.worker.runtime
  const reviewerRuntime = reviewer.worker.runtime
  if (!implementerRuntime || !reviewerRuntime) {
    throw new Error('Workspace navigator runtimes are missing')
  }
  state.runtimeInventory.workers.push(
    {
      ...worker(20, 'workspace-1', 'working'),
      runtime_id: implementerRuntime.terminal_id,
      terminal_id: implementerRuntime.terminal_id,
      tab_id: implementerRuntime.tab_id ?? 'workspace-1:tab-implementer',
      pane_id: implementerRuntime.pane_id,
      name: 'implementer',
      cwd: '/tmp/sample/yard-worktrees/api-migration/web',
      foreground_cwd: '/tmp/sample/yard-worktrees/api-migration/web',
    },
    {
      ...worker(21, 'workspace-2', 'blocked'),
      runtime_id: reviewerRuntime.terminal_id,
      terminal_id: reviewerRuntime.terminal_id,
      tab_id: reviewerRuntime.tab_id ?? 'workspace-2:tab-reviewer',
      pane_id: reviewerRuntime.pane_id,
      name: 'reviewer',
      cwd: '/tmp/sample/release-tools/checks',
      foreground_cwd: '/tmp/sample/release-tools/checks',
    },
  )

  await page.addInitScript(() => {
    window.localStorage.setItem('yard:theme', 'light')
  })
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page
    .locator(
      `.assigned-worker-marker[data-worker-id="${implementer.worker.id}"]`,
    )
    .click()
  const openChat = page.getByRole('button', {
    name: 'Open chat',
    exact: true,
  })
  await openChat.click()

  const shell = page.locator('.agent-workspace-shell')
  const navigator = shell.getByLabel('Herdr windows')
  await expect(shell).toBeVisible()
  await expect(navigator).toContainText('Runtime working')
  await expect(navigator).toContainText('durable-only')
  await expect(navigator).toContainText('release-tools')
  await expect(navigator).toContainText('Topology not observed')
  const workspaceGroups = navigator.locator('.agent-window-workspace')
  await expect(workspaceGroups).toHaveCount(3)
  await expect(workspaceGroups.nth(0)).toHaveAttribute(
    'data-workspace-id',
    'workspace-2',
  )
  await expect(workspaceGroups.nth(1)).toHaveAttribute(
    'data-workspace-id',
    'workspace-1',
  )
  await expect(workspaceGroups.nth(2)).toHaveAttribute(
    'data-workspace-id',
    'workspace-offline',
  )
  await expect(workspaceGroups.nth(1)).toContainText('API migration')
  await expect(workspaceGroups.nth(1)).toContainText('workspace-1')
  await expect(workspaceGroups.nth(1)).toContainText('2 targets')
  await expect(workspaceGroups.nth(1)).toContainText('6 tabs · 6 panes')
  await expect(workspaceGroups.nth(1)).toContainText('Focused')
  await expect(workspaceGroups.nth(1)).toContainText(
    '/tmp/sample/yard-worktrees/api-migration',
  )
  await expect(workspaceGroups.nth(2)).toContainText('Session offline')
  await expect(workspaceGroups.nth(2)).toContainText(
    'workspace-offline',
  )
  const targetRows = navigator.locator('.agent-window-row')
  await expect(targetRows).toHaveCount(4)
  const targetKeys = await targetRows.evaluateAll((rows) =>
    rows.map((row) => row.getAttribute('data-target-key')),
  )
  expect(new Set(targetKeys).size).toBe(4)
  for (const key of targetKeys) {
    await expect(
      navigator.locator(`[data-target-key="${key}"]`),
    ).toHaveCount(1)
  }
  await expect(workspaceGroups.nth(1).locator('.agent-window-row')).toHaveCount(
    2,
  )
  const implementerRow = navigator.locator(
    '[data-target-key="assignment:assignment-1"]',
  )
  await expect(implementerRow).toHaveAttribute('aria-current', 'page')
  await expect(implementerRow).toContainText('Implementer')
  await expect(implementerRow).toContainText('implementer · API migration')
  await expect(implementerRow).toContainText('Claude · alpha')
  await expect(implementerRow).toContainText(
    'tab workspace-1:tab-assignment-1-terminal',
  )
  await expect(implementerRow).toContainText(
    '/tmp/sample/yard-worktrees/api-migration/web',
  )
  const implementerDescriptionId = await implementerRow.getAttribute(
    'aria-describedby',
  )
  expect(implementerDescriptionId).not.toBeNull()
  const implementerDescription = page.locator(
    `#${implementerDescriptionId}`,
  )
  await expect(implementerDescription).toContainText(
    'Observed runtime: working',
  )
  await expect(implementerDescription).toContainText(
    'herdr session alpha',
  )
  await expect(implementerDescription).toContainText(
    'pane workspace-1:pane-assignment-1-terminal',
  )
  await expect(page.locator('.modal-backdrop.chat-backdrop')).toHaveCount(0)
  const shellBounds = await shell.boundingBox()
  expect(shellBounds).not.toBeNull()
  expect(shellBounds?.x).toBe(0)
  expect(shellBounds ? shellBounds.x + shellBounds.width : 0).toBe(1280)
  expect(shellBounds ? shellBounds.y + shellBounds.height : 0).toBe(800)
  const desktopLayout = await page.evaluate(() => {
    const nav = document.querySelector<HTMLElement>(
      '.agent-window-navigator',
    )
    const content = document.querySelector<HTMLElement>(
      '.agent-workspace-content',
    )
    return {
      contentWidth: content?.getBoundingClientRect().width ?? 0,
      documentOverflow:
        document.documentElement.scrollWidth - window.innerWidth,
      navigatorOverflow: nav ? nav.scrollWidth - nav.clientWidth : 1,
    }
  })
  expect(desktopLayout.contentWidth).toBeGreaterThanOrEqual(900)
  expect(desktopLayout.documentOverflow).toBeLessThanOrEqual(0)
  expect(desktopLayout.navigatorOverflow).toBeLessThanOrEqual(0)

  await page
    .getByRole('tab', { name: 'Terminal', exact: true })
    .click()
  await expect(page.locator('.terminal-session')).toHaveAttribute(
    'data-state',
    'connected',
  )
  await expect(page.locator('.modal-backdrop.terminal-backdrop')).toHaveCount(
    0,
  )
  const reviewerRow = navigator.locator(
    '[data-target-key="assignment:assignment-reviewer"]',
  )
  const keyboardFocusRow = navigator.locator(
    '[data-target-key="orchestrator:project-1"]',
  )
  await reviewerRow.focus()
  await page.keyboard.press('Tab')
  await expect(keyboardFocusRow).toBeFocused()
  expect(
    await keyboardFocusRow.evaluate((element) =>
      element.matches(':focus-visible'),
    ),
  ).toBe(true)
  await navigator.evaluate((element) => {
    const probe = document.createElement('span')
    probe.dataset.contrastProbe = 'alpha'
    probe.textContent = 'Alpha contrast probe'
    probe.style.cssText =
      'position:fixed;left:0;top:0;display:block;color:rgba(0,0,0,0.5);background:rgb(255,255,255)'
    element.append(probe)
  })
  const alphaContrasts = await navigatorMetadataContrasts(navigator)
  const alphaSample = alphaContrasts.find((sample) =>
    sample.label.includes('Alpha contrast probe'),
  )
  expect(alphaSample).toBeDefined()
  expect(alphaSample?.foregroundAlpha).toBeCloseTo(128 / 255, 5)
  expect(alphaSample?.sourceForeground).toEqual([0, 0, 0])
  expect(alphaSample?.foreground[0]).toBeCloseTo(127, 0)
  expect(alphaSample?.foreground[1]).toBeCloseTo(127, 0)
  expect(alphaSample?.foreground[2]).toBeCloseTo(127, 0)
  expect(alphaSample?.contrast).toBeCloseTo(4.004, 2)
  expect(alphaSample?.contrast ?? Infinity).toBeLessThan(4.5)
  await navigator
    .locator('[data-contrast-probe]')
    .evaluate((element) => element.remove())
  const lightContrasts = await navigatorMetadataContrasts(navigator)
  expect(lightContrasts.length).toBeGreaterThan(10)
  expect(
    Math.min(...lightContrasts.map((sample) => sample.contrast)),
  ).toBeGreaterThanOrEqual(4.5)
  await page.screenshot({
    path: testInfo.outputPath('workspace-navigator-light-desktop.png'),
    fullPage: true,
  })
  await setAppTheme(page, 'Dark')
  await reviewerRow.focus()
  await page.keyboard.press('Tab')
  await expect(keyboardFocusRow).toBeFocused()
  const darkContrasts = await navigatorMetadataContrasts(navigator)
  expect(darkContrasts.length).toBe(lightContrasts.length)
  expect(
    Math.min(...darkContrasts.map((sample) => sample.contrast)),
  ).toBeGreaterThanOrEqual(4.5)
  await page.screenshot({
    path: testInfo.outputPath('workspace-navigator-dark-desktop.png'),
    fullPage: true,
  })
  await page.getByRole('tab', { name: 'Chat', exact: true }).click()
  await expect
    .poll(() =>
      state.terminalMessages.some(
        (message) => message.type === 'terminal.release',
      ),
    )
    .toBe(true)

  await navigator
    .getByRole('button', { name: /API migration orchestrator/ })
    .click()
  await expect(implementerRow).not.toHaveAttribute('aria-current', 'page')
  await expect(
    navigator.locator('[data-target-key="orchestrator:project-1"]'),
  ).toHaveAttribute('aria-current', 'page')
  await expect(shell).toContainText('API migration orchestrator')
  await expect(
    page.getByRole('tab', { name: 'Terminal', exact: true }),
  ).toHaveAttribute('aria-selected', 'true')
  await expect(page.locator('.terminal-session')).toHaveAttribute(
    'data-state',
    'connected',
  )
  await page.getByRole('tab', { name: 'Chat', exact: true }).click()
  await expect(page.getByLabel('Agent conversation')).toBeVisible()

  await page.setViewportSize({ width: 390, height: 844 })
  const mobileBounds = await shell.boundingBox()
  expect(mobileBounds?.x).toBe(0)
  expect(mobileBounds ? mobileBounds.x + mobileBounds.width : 0).toBe(390)
  expect(mobileBounds ? mobileBounds.y + mobileBounds.height : 0).toBe(844)
  const mobileLayout = await page.evaluate(() => {
    const nav = document.querySelector<HTMLElement>(
      '.agent-window-navigator',
    )
    const content = document.querySelector<HTMLElement>(
      '.agent-workspace-content',
    )
    return {
      contentWidth: content?.getBoundingClientRect().width ?? 0,
      documentOverflow:
        document.documentElement.scrollWidth - window.innerWidth,
      navigatorOverflow: nav ? nav.scrollWidth - nav.clientWidth : 1,
      shellOverflow:
        document.querySelector<HTMLElement>('.agent-workspace-shell')
          ?.scrollWidth ?? 1,
      shellWidth:
        document.querySelector<HTMLElement>('.agent-workspace-shell')
          ?.clientWidth ?? 0,
    }
  })
  expect(mobileLayout.contentWidth).toBeGreaterThanOrEqual(250)
  expect(mobileLayout.documentOverflow).toBeLessThanOrEqual(0)
  expect(mobileLayout.navigatorOverflow).toBeLessThanOrEqual(0)
  expect(mobileLayout.shellOverflow).toBeLessThanOrEqual(
    mobileLayout.shellWidth,
  )
  await page.screenshot({
    path: testInfo.outputPath('workspace-navigator-dark-mobile.png'),
    fullPage: true,
  })
  await setAppTheme(page, 'Light')
  await page.screenshot({
    path: testInfo.outputPath('workspace-navigator-light-mobile.png'),
    fullPage: true,
  })

  await page.setViewportSize({ width: 320, height: 844 })
  const implementerDetails = navigator.getByRole('button', {
    name: 'Show runtime details for Implementer, API migration, workspace workspace-1, alpha terminal assignment-1-terminal',
    exact: true,
  })
  const reviewerDetails = navigator.getByRole('button', {
    name: 'Show runtime details for Implementer, API migration, workspace workspace-2, alpha terminal terminal-reviewer',
    exact: true,
  })
  await expect(implementerRow.locator('strong')).toHaveText('Implementer')
  await expect(reviewerRow.locator('strong')).toHaveText('Implementer')
  await expect(implementerDetails).toBeVisible()
  await expect(reviewerDetails).toBeVisible()
  expect(await implementerDetails.getAttribute('aria-label')).not.toBe(
    await reviewerDetails.getAttribute('aria-label'),
  )
  await expect(reviewerDetails).toHaveAttribute('aria-haspopup', 'dialog')
  const detailsBounds = await reviewerDetails.boundingBox()
  expect(detailsBounds?.width ?? 0).toBeGreaterThanOrEqual(32)
  expect(detailsBounds?.height ?? 0).toBeGreaterThanOrEqual(44)
  const activeKeyBeforeDetails = await navigator
    .locator('.agent-window-row[aria-current="page"]')
    .getAttribute('data-target-key')
  await page.screenshot({
    path: testInfo.outputPath(
      'workspace-navigator-light-320-duplicate-groups.png',
    ),
    fullPage: true,
  })

  await implementerDetails.click()
  const implementerDetailsDialog = page.getByRole('dialog', {
    name: 'Implementer',
  })
  await expect(implementerDetailsDialog).toContainText('workspace-1')
  await expect(implementerDetailsDialog).toContainText(
    'assignment-1-terminal',
  )
  await page.getByRole('button', { name: 'Close runtime details' }).click()
  await expect(implementerDetails).toBeFocused()

  await reviewerDetails.click()
  const detailsDialog = page.getByRole('dialog', {
    name: 'Implementer',
  })
  await expect(detailsDialog).toBeVisible()
  await expect(detailsDialog).toContainText('workspace-2')
  await expect(detailsDialog).toContainText('terminal-reviewer')
  await expect(detailsDialog).toContainText(
    'workspace-2:tab-terminal-reviewer',
  )
  await expect(detailsDialog).toContainText(
    'workspace-2:pane-terminal-reviewer',
  )
  await expect(
    navigator.locator('.agent-window-row[aria-current="page"]'),
  ).toHaveAttribute('data-target-key', activeKeyBeforeDetails ?? '')
  const detailsBackdrop = page.locator(
    'body > .agent-window-details-backdrop',
  )
  const commandBar = page.locator('.command-bar')
  await expect(detailsBackdrop).toBeVisible()
  const stacking = await page.evaluate(() => {
    const backdrop = document.querySelector<HTMLElement>(
      'body > .agent-window-details-backdrop',
    )
    const commandBar = document.querySelector<HTMLElement>('.command-bar')
    if (!backdrop || !commandBar) return null
    const bounds = commandBar.getBoundingClientRect()
    const hit = document.elementFromPoint(
      bounds.left + bounds.width / 2,
      bounds.top + bounds.height / 2,
    )
    return {
      backdropZIndex: Number(getComputedStyle(backdrop).zIndex),
      commandBarZIndex: Number(getComputedStyle(commandBar).zIndex),
      hitBackdrop: hit === backdrop,
      outsideAriaHidden: Boolean(commandBar.closest('[aria-hidden="true"]')),
      outsideInert: Boolean(commandBar.closest('[inert]')),
    }
  })
  expect(stacking).toEqual({
    backdropZIndex: 150,
    commandBarZIndex: 130,
    hitBackdrop: true,
    outsideAriaHidden: true,
    outsideInert: true,
  })
  const detailsOverflow = await detailsDialog.evaluate((element) => ({
    dialog: element.scrollWidth - element.clientWidth,
    document:
      document.documentElement.scrollWidth -
      document.documentElement.clientWidth,
    shell:
      (document.querySelector<HTMLElement>('.agent-workspace-shell')
        ?.scrollWidth ?? 1) -
      (document.querySelector<HTMLElement>('.agent-workspace-shell')
        ?.clientWidth ?? 0),
  }))
  expect(detailsOverflow.dialog).toBeLessThanOrEqual(0)
  expect(detailsOverflow.document).toBeLessThanOrEqual(0)
  expect(detailsOverflow.shell).toBeLessThanOrEqual(0)
  await page.screenshot({
    path: testInfo.outputPath(
      'workspace-navigator-light-320-runtime-details.png',
    ),
    fullPage: true,
  })
  await page.keyboard.press('Escape')
  await expect(detailsDialog).toHaveCount(0)
  await expect(reviewerDetails).toBeFocused()
  await expect(commandBar).not.toHaveAttribute('aria-hidden', 'true')
  expect(
    await commandBar.evaluate((element) =>
      Boolean(element.closest('[inert], [aria-hidden="true"]')),
    ),
  ).toBe(false)

  const narrowLayout = await page.evaluate(() => ({
    document:
      document.documentElement.scrollWidth -
      document.documentElement.clientWidth,
    navigator:
      (document.querySelector<HTMLElement>('.agent-window-navigator')
        ?.scrollWidth ?? 1) -
      (document.querySelector<HTMLElement>('.agent-window-navigator')
        ?.clientWidth ?? 0),
    shell:
      (document.querySelector<HTMLElement>('.agent-workspace-shell')
        ?.scrollWidth ?? 1) -
      (document.querySelector<HTMLElement>('.agent-workspace-shell')
        ?.clientWidth ?? 0),
  }))
  expect(narrowLayout.document).toBeLessThanOrEqual(0)
  expect(narrowLayout.navigator).toBeLessThanOrEqual(0)
  expect(narrowLayout.shell).toBeLessThanOrEqual(0)
  await shell.getByRole('button', { name: 'Close chat' }).click()
  await expect(shell).toBeHidden()
  await expect(openChat).toBeFocused()
})

test('uses reported workflow status on the canvas and keeps runtime state distinct', async ({
  page,
}, testInfo) => {
  const longLast = `Validated the compatibility layer. ${'last-segment '.repeat(180)}LAST-END`
  const longNext = `Run the focused migration suite. ${'next-segment '.repeat(180)}NEXT-END`
  const longReportBlocker = `Signing credential is unavailable. ${'blocker-segment '.repeat(110)}BLOCKER-END`
  const completionBlocker =
    'Resolve the release approval before accepting completion.'
  const validReport: StatusReport = {
    version: 1,
    command_id: 'direct-valid',
    state: 'working',
    last: '  whitespace in last is preserved  ',
    next: '',
    blockers: [],
  }
  expect(parseStatusReport(validReport)).toEqual(validReport)

  const reportLineBytes = (report: unknown) =>
    new TextEncoder().encode(JSON.stringify(report)).byteLength
  const maximumReport = {
    ...validReport,
    last: 'l'.repeat(4096),
    next: 'n'.repeat(4096),
    blockers: [
      'b'.repeat(2048),
      'c'.repeat(2048),
      'd'.repeat(2048),
    ],
  }
  const remainingBlockerBytes =
    16_384 - reportLineBytes(maximumReport) - 3
  maximumReport.blockers.push('e'.repeat(remainingBlockerBytes))
  expect(reportLineBytes(maximumReport)).toBe(16_384)
  expect(parseStatusReport(maximumReport)).toEqual(maximumReport)
  const oversizedReport = {
    ...maximumReport,
    blockers: [
      ...maximumReport.blockers.slice(0, -1),
      `${maximumReport.blockers.at(-1)}e`,
    ],
  }
  expect(reportLineBytes(oversizedReport)).toBe(16_385)
  expect(parseStatusReport(oversizedReport)).toBeNull()

  ;[
    { ...validReport, extra: true },
    { ...validReport, command_id: ' padded-command ' },
    { ...validReport, command_id: 'é'.repeat(61) },
    { ...validReport, last: 'é'.repeat(2049) },
    { ...validReport, next: 'é'.repeat(2049) },
    { ...validReport, blockers: Array(33).fill('blocked') },
    { ...validReport, blockers: [''] },
    { ...validReport, blockers: [' padded blocker '] },
    { ...validReport, blockers: ['é'.repeat(1025)] },
    { ...validReport, state: 'done' },
    { ...validReport, version: 2 },
  ].forEach((candidate) => expect(parseStatusReport(candidate)).toBeNull())

  const reports: Record<string, unknown> = {
    'project-1': {
      version: 1,
      command_id: 'route-project-1-failed',
      state: 'working',
      last: 'Validated the compatibility layer.',
      next: 'Run the focused migration suite.',
      blockers: [],
    },
    'project-2': {
      version: 1,
      command_id: 'newer-direct-project-2',
      state: 'idle',
      last: longLast,
      next: longNext,
      blockers: [longReportBlocker],
    },
    'project-3': {
      version: 1,
      command_id: 'direct-project-3',
      state: 'working',
      last: 'The implementation report says work is progressing.',
      next: 'Continue implementation.',
      blockers: [],
    },
    'project-4': {
      version: 1,
      command_id: 'direct-project-4',
      state: 'idle',
      last: 'The report says this project is waiting.',
      next: 'Wait for more work.',
      blockers: [],
    },
    'project-5': {
      version: 1,
      command_id: 'direct-project-5',
      state: 'working',
      last: 'The orchestrator report says it is working.',
      next: 'Continue orchestration.',
      blockers: [],
    },
    'project-6': {
      version: 1,
      command_id: 'route-project-6-old',
      state: 'working',
      last: 'This stale route report must not render.',
      next: 'This stale next step must not render.',
      blockers: [],
    },
    'project-7': {
      version: 1,
      command_id: 'invalid-extra-field',
      state: 'working',
      last: 'This invalid report must not render.',
      next: 'This invalid next step must not render.',
      blockers: [],
      extra: 'rejected',
    },
  }
  const state = await mockApi(page, {
    orchestratorStatusReports: reports,
  })
  state.projects.push(
    project(
      'project-3',
      'Search indexing',
      'alpha',
      'workspace-3',
      'terminal-8',
      860,
    ),
    project(
      'project-4',
      'Docs refresh',
      'alpha',
      'workspace-4',
      'terminal-9',
      1250,
    ),
    project(
      'project-5',
      'Runtime audit',
      'alpha',
      'workspace-5',
      'terminal-10',
      1640,
    ),
    project(
      'project-6',
      'Route freshness',
      'alpha',
      'workspace-6',
      'terminal-11',
      2030,
    ),
    project(
      'project-7',
      'Strict fallback',
      'alpha',
      'workspace-7',
      'terminal-12',
      2420,
    ),
  )
  const now = Date.now()
  state.yardOrchestratorRoutes.push(
    {
      command_id: 'route-project-1-failed',
      actor: 'local-user',
      orchestrator_worker_id: 'yard-worker',
      expected_orchestrator_version: '1',
      target_project_id: 'project-1',
      target_orchestrator_worker_id: 'project-1-orchestrator',
      expected_project_version: '1',
      text: 'Retry the compatibility route.',
      status: 'failed',
      error_message: 'Retry the compatibility route after fixing access.',
      runtime_status: null,
      created_at_unix_ms: now,
      updated_at_unix_ms: now,
      submitted_at_unix_ms: null,
    },
    {
      command_id: 'route-project-6-current',
      actor: 'local-user',
      orchestrator_worker_id: 'yard-worker',
      expected_orchestrator_version: '1',
      target_project_id: 'project-6',
      target_orchestrator_worker_id: 'project-6-orchestrator',
      expected_project_version: '1',
      text: 'Use the current route.',
      status: 'submitted',
      error_message: null,
      runtime_status: 'accepted',
      created_at_unix_ms: now,
      updated_at_unix_ms: now,
      submitted_at_unix_ms: now,
    },
    {
      command_id: 'route-project-6-old',
      actor: 'local-user',
      orchestrator_worker_id: 'yard-worker',
      expected_orchestrator_version: '1',
      target_project_id: 'project-6',
      target_orchestrator_worker_id: 'project-6-orchestrator',
      expected_project_version: '1',
      text: 'Use the obsolete route.',
      status: 'submitted',
      error_message: null,
      runtime_status: 'accepted',
      created_at_unix_ms: now - 1_000,
      updated_at_unix_ms: now - 1_000,
      submitted_at_unix_ms: now - 1_000,
    },
  )
  const staleReport = parseStatusReport(reports['project-6'])
  const directReport = parseStatusReport(reports['project-2'])
  expect(staleReport).not.toBeNull()
  expect(directReport).not.toBeNull()
  expect(
    statusReportMatchesLatestRoute(
      staleReport!,
      'project-6',
      state.yardOrchestratorRoutes,
    ),
  ).toBe(false)
  expect(
    statusReportMatchesLatestRoute(
      directReport!,
      'project-2',
      state.yardOrchestratorRoutes,
    ),
  ).toBe(true)
  expect(
    statusReportMatchesLatestRoute(
      parseStatusReport(reports['project-1'])!,
      'project-2',
      state.yardOrchestratorRoutes,
    ),
  ).toBe(false)
  const failedRoute = state.yardOrchestratorRoutes.find(
    (route) => route.command_id === 'route-project-1-failed',
  )!
  const nonAuthoritativeRoutes: YardOrchestratorRoute[] = [
    failedRoute,
    {
      ...failedRoute,
      command_id: 'route-project-1-ambiguous',
      status: 'ambiguous',
    },
    {
      ...failedRoute,
      command_id: 'route-project-1-pending',
      status: 'pending',
    },
    {
      ...failedRoute,
      command_id: 'route-project-1-missing-runtime-status',
      status: 'submitted',
      submitted_at_unix_ms: now,
    },
    {
      ...failedRoute,
      command_id: 'route-project-1-missing-submitted-at',
      status: 'submitted',
      runtime_status: 'accepted',
    },
  ]
  for (const route of nonAuthoritativeRoutes) {
    expect(
      statusReportMatchesLatestRoute(
        { ...validReport, command_id: route.command_id },
        'project-1',
        [route],
      ),
    ).toBe(false)
  }
  const deliveredRoute = state.yardOrchestratorRoutes.find(
    (route) => route.command_id === 'route-project-6-current',
  )!
  expect(
    statusReportMatchesLatestRoute(
      { ...validReport, command_id: deliveredRoute.command_id },
      'project-6',
      [
        deliveredRoute,
        {
          ...deliveredRoute,
          command_id: 'route-project-6-newer-failed',
          status: 'failed',
          runtime_status: null,
          submitted_at_unix_ms: null,
          updated_at_unix_ms: now + 1_000,
        },
      ],
    ),
  ).toBe(true)

  const doneAssignment = assignment(
    'assignment-done-active',
    'project-3',
    state.profiles[0],
    'Finish the indexing migration.',
    'implementer',
  )
  doneAssignment.worker.runtime!.status = 'done'
  const completedAssignment = assignment(
    'assignment-completion-blocked',
    'project-4',
    state.profiles[0],
    'Publish the documentation refresh.',
    'implementer',
  )
  completedAssignment.lifecycle = 'completed'
  completedAssignment.attempt.lifecycle = 'completed'
  completedAssignment.completion_receipt = {
    id: 'receipt-completion-blocked',
    assignment_id: completedAssignment.id,
    attempt_id: completedAssignment.attempt.id,
    outcome: 'completed',
    summary: 'Prepared the documentation refresh.',
    artifact_refs: [],
    artifacts: [],
    evidence_refs: [],
    unresolved_blockers: [completionBlocker],
    actor: 'local-user',
    created_at_unix_ms: now,
  }
  state.assignments.push(doneAssignment, completedAssignment)
  state.projects.find(
    (candidate) => candidate.id === 'project-5',
  )!.orchestrator.runtime!.status = 'blocked'

  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  const pulseTrigger = page.getByRole('button', { name: 'Project pulse' })
  await expect(page.getByLabel('Latest project updates')).toHaveCount(0)
  await pulseTrigger.click()
  const pulseDialog = page.getByRole('dialog', { name: 'Project pulse' })
  const pulse = pulseDialog.getByLabel('Latest project updates')
  await expect(pulseDialog).toBeVisible()
  const closePulse = pulseDialog.getByRole('button', {
    name: 'Close Project pulse',
  })
  await expect(closePulse).toBeFocused()
  await expect(page.locator('.app-shell')).toHaveAttribute(
    'aria-hidden',
    'true',
  )
  await expect(page.locator('.app-shell')).toHaveAttribute('inert', '')
  await expect(
    pulseDialog.getByRole('heading', { name: 'Project pulse' }),
  ).toHaveCount(1)
  await expect(pulse).toBeVisible()
  await expect(page.locator('.inspector')).toHaveCount(0)
  await expect
    .poll(
      () =>
        state.orchestratorTerminalOutputRequests.filter(
          (request) => request.lines === '80',
        ).length,
    )
    .toBeGreaterThanOrEqual(state.projects.length)

  await page.keyboard.press('Shift+Tab')
  await expect(pulse.locator('.yard-project-update').last()).toBeFocused()
  await page.keyboard.press('Tab')
  await expect(closePulse).toBeFocused()

  const routeAttentionUpdate = pulse.locator(
    '.yard-project-update[data-project-id="project-1"]',
  )
  await expect(routeAttentionUpdate).toHaveAttribute(
    'data-source',
    'durable',
  )
  await expect(routeAttentionUpdate).toHaveAttribute(
    'data-state',
    'needs_action',
  )
  await expect(routeAttentionUpdate).not.toHaveAttribute(
    'data-workflow-state',
    'working',
  )
  await expect(routeAttentionUpdate).toContainText('Needs you')
  await expect(routeAttentionUpdate.locator('[data-field="last"]')).toHaveText(
    'Directed: Retry the compatibility route.',
  )
  await expect(routeAttentionUpdate).not.toContainText(
    'Validated the compatibility layer.',
  )
  await expect(routeAttentionUpdate.locator('[data-field="next"]')).toHaveText(
    'Retry the compatibility route after fixing access.',
  )

  const longUpdate = pulse.locator(
    '.yard-project-update[data-project-id="project-2"]',
  )
  await expect(longUpdate).toHaveAttribute('data-source', 'status_report')
  await expect(longUpdate.locator('[data-field="last"]')).toHaveText(longLast)
  await expect(longUpdate.locator('[data-field="next"]')).toHaveText(longNext)
  await expect(longUpdate.locator('[data-field="blocker"]')).toHaveText(
    longReportBlocker,
  )
  await expect(longUpdate).toContainText('Idle')
  await expect(longUpdate).not.toContainText('Completed')

  const doneAttention = pulse.locator(
    '.yard-project-update[data-project-id="project-3"]',
  )
  await expect(doneAttention).toHaveAttribute('data-state', 'needs_action')
  await expect(doneAttention.locator('[data-field="next"]')).toContainText(
    'Review Implementer (done): Finish the indexing migration.',
  )
  const completionAttention = pulse.locator(
    '.yard-project-update[data-project-id="project-4"]',
  )
  await expect(completionAttention).toHaveAttribute(
    'data-state',
    'needs_action',
  )
  await expect(
    completionAttention.locator('[data-field="next"]'),
  ).toContainText(`Resolve completion blocker: ${completionBlocker}`)
  await expect(
    completionAttention.locator('[data-field="blocker"]'),
  ).toHaveText(completionBlocker)
  const orchestratorAttention = pulse.locator(
    '.yard-project-update[data-project-id="project-5"]',
  )
  await expect(orchestratorAttention).toHaveAttribute(
    'data-state',
    'needs_action',
  )
  await expect(
    orchestratorAttention.locator('[data-field="next"]'),
  ).toHaveText(
    'Open the project orchestrator and resolve its blocked runtime.',
  )
  const staleUpdate = pulse.locator(
    '.yard-project-update[data-project-id="project-6"]',
  )
  await expect(staleUpdate).toHaveAttribute('data-source', 'durable')
  await expect(staleUpdate).not.toContainText(
    'This stale route report must not render.',
  )
  const invalidUpdate = pulse.locator(
    '.yard-project-update[data-project-id="project-7"]',
  )
  await expect(invalidUpdate).toHaveAttribute('data-source', 'durable')
  await expect(invalidUpdate).not.toContainText(
    'This invalid report must not render.',
  )

  const marker = page.locator(
    '.orchestrator-marker[data-project-id="project-1"]',
  )
  await expect(marker).toHaveAttribute('data-status', 'idle')
  await expect(marker).toHaveAttribute(
    'data-workflow-state',
    'unreported',
  )
  await expect(marker.locator('.worker-marker__workflow')).toContainText(
    'Orchestrator',
  )
  await longUpdate.scrollIntoViewIfNeeded()
  const clipping = await longUpdate
    .locator('[data-field]')
    .evaluateAll((elements) =>
      elements.map((element) => ({
        horizontal: element.scrollWidth - element.clientWidth,
        overflow: getComputedStyle(element).overflow,
        vertical: element.scrollHeight - element.clientHeight,
      })),
    )
  expect(
    clipping.every(
      (value) =>
        value.horizontal <= 1 &&
        value.vertical <= 1 &&
        value.overflow !== 'hidden',
    ),
  ).toBe(true)
  const desktopOverflow = await pulseDialog.evaluate((element) => ({
    dialog: element.scrollWidth - element.clientWidth,
    row:
      element.querySelector<HTMLElement>(
        '[data-project-id="project-2"]',
      )!.scrollWidth -
      element.querySelector<HTMLElement>(
        '[data-project-id="project-2"]',
      )!.clientWidth,
  }))
  expect(desktopOverflow.dialog).toBeLessThanOrEqual(0)
  expect(desktopOverflow.row).toBeLessThanOrEqual(1)
  await page.screenshot({
    path: testInfo.outputPath('project-pulse-long-message.png'),
    fullPage: true,
  })
  await page.keyboard.press('Escape')
  await expect(pulseDialog).toHaveCount(0)
  await expect(pulseTrigger).toBeFocused()
  await expect(page.locator('.app-shell')).not.toHaveAttribute(
    'aria-hidden',
    'true',
  )
  await expect(page.locator('.app-shell')).not.toHaveAttribute('inert', '')

  await page
    .locator('[data-id="project:project-1"]')
    .dispatchEvent('click')
  const projectWorkflow = page.getByRole('region', {
    name: 'Reported workflow status',
  })
  await expect(projectWorkflow).toHaveCount(0)
  await expect(
    page.locator('.project-workspace-state .status-badge'),
  ).toHaveAttribute('data-status', 'idle')

  await page.getByRole('button', { name: 'Close details' }).click()
  await marker.click()
  await expect(
    page.getByRole('region', { name: 'Reported workflow status' }),
  ).toHaveCount(0)
  await expect(
    page.getByRole('region', { name: 'Orchestrator runtime' }),
  ).toContainText('Observed status')

  await page.getByRole('button', { name: 'Close details' }).click()
  await page.setViewportSize({ width: 390, height: 844 })
  await pulseTrigger.click()
  await expect(pulseDialog).toBeVisible()
  await expect(closePulse).toBeFocused()
  const overflow = await pulseDialog.evaluate((element) => ({
    horizontal: element.scrollWidth - element.clientWidth,
    right: element.getBoundingClientRect().right - window.innerWidth,
    vertical: element.getBoundingClientRect().bottom - window.innerHeight,
  }))
  expect(overflow.horizontal).toBeLessThanOrEqual(0)
  expect(overflow.right).toBeLessThanOrEqual(0)
  expect(overflow.vertical).toBeLessThanOrEqual(0)
  await page.screenshot({
    path: testInfo.outputPath('project-pulse-mobile.png'),
    fullPage: true,
  })
  await closePulse.click()
  await expect(pulseTrigger).toBeFocused()
})

test('creates a project-targeted automation and persists its satellite placement', async ({
  page,
}, testInfo) => {
  const state = await mockApi(page)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page.locator('.orchestrator-marker').first().click({
    button: 'right',
  })
  await page.getByRole('menuitem', { name: /Automation/ }).click()

  await expect(page.getByLabel('Target orchestrator')).toHaveValue(
    'project:project-1',
  )
  const owningProject = page.getByRole('checkbox', {
    name: 'API migration',
  })
  await expect(owningProject).toBeChecked()
  await expect(owningProject).toBeDisabled()
  await expect(
    page.getByRole('checkbox', { name: 'Offline release' }),
  ).toHaveCount(0)

  await page.getByLabel('Name').fill('Daily migration review')
  await page
    .getByLabel('Prompt template')
    .fill('Review the selected migration project and report blockers.')
  await page
    .getByRole('button', { name: 'Create automation' })
    .click()

  const satellite = page.locator('.automation-map-node')
  await expect(satellite).toContainText('Daily migration review')
  await expect(satellite).toContainText('Active')
  await expect(page.locator('.automation-target-edge')).toHaveCount(1)
  expect(state.automationCreateCommands).toHaveLength(1)
  expect(state.automationCreateCommands[0]).toMatchObject({
    name: 'Daily migration review',
    scope: {
      kind: 'project_orchestrator',
      project_id: 'project-1',
    },
    placement: {
      height: 58,
      width: 168,
    },
    selected_project_ids: ['project-1'],
    prompt_template:
      'Review the selected migration project and report blockers.',
  })
  expect(state.automationCreateCommands[0].schedule.hour).toBe(9)
  expect(state.automationCreateCommands[0].schedule.minute).toBe(0)
  expect(state.automationCreateCommands[0].schedule.timezone).toBeTruthy()
  expect(state.automationCreateCommands[0].placement.y).toBe(8)
  await page.screenshot({
    path: testInfo.outputPath('automation-project-target.png'),
    fullPage: true,
  })

  await page.getByRole('button', { name: 'Close details' }).click()
  const node = page.locator('.react-flow__node-automation')
  const handle = node.locator('.automation-map-node__grip')
  const handleBox = await handle.boundingBox()
  if (!handleBox) throw new Error('Automation placement grip is hidden')
  await page.mouse.move(
    handleBox.x + handleBox.width / 2,
    handleBox.y + handleBox.height / 2,
  )
  await page.mouse.down()
  await page.mouse.move(
    handleBox.x + handleBox.width / 2 + 90,
    handleBox.y + handleBox.height / 2 + 36,
    { steps: 8 },
  )
  await page.mouse.up()
  await expect
    .poll(() => state.automationPlacementUpdates, { timeout: 5_000 })
    .toBe(1)
  expect(state.automationPlacementCommands[0].expected_version).toBe('1')
  const persisted = state.automations[0].placement.geometry
  await page.reload()
  await expect(page.locator('.automation-map-node')).toBeVisible()
  expect(state.automations[0].placement.geometry).toEqual(persisted)
})

test('persists independent automatic token-use settings while keeping manual actions separate', async ({
  page,
}, testInfo) => {
  const state = await mockApi(page)
  await page.goto('/')

  const openAutomaticCoordination = async () => {
    const settings = await openSettings(page)
    const settingsButton = settings.getByRole('button', {
      name: 'Automatic token use settings',
    })
    await expect(settingsButton).toBeEnabled()
    await settingsButton.click()
    return page.getByRole('dialog', {
      name: 'Coordination settings',
    })
  }
  const dialog = await openAutomaticCoordination()
  const superintendent = dialog.getByRole('switch', {
    name: /Superintendent to project orchestrators/,
  })
  const projectOrchestrators = dialog.getByRole('switch', {
    name: /Project orchestrators to workers/,
  })
  const scheduled = dialog.getByRole('switch', {
    name: /Scheduled automatic summaries/,
  })
  await expect(superintendent).not.toBeChecked()
  await expect(projectOrchestrators).not.toBeChecked()
  await expect(scheduled).not.toBeChecked()
  await expect(dialog.getByText('Manual actions')).toBeVisible()

  await superintendent.check()
  await dialog.getByRole('button', {
    name: 'Save automatic settings',
  }).click()

  expect(state.tokenSpendSettingsUpdates).toHaveLength(1)
  expect(state.tokenSpendSettingsUpdates[0]).toMatchObject({
    superintendent_auto_requests_project_summaries: true,
    project_orchestrators_auto_request_worker_summaries: false,
    scheduled_automatic_summaries: false,
  })
  await expect(dialog).toBeHidden()

  const reopened = await openAutomaticCoordination()
  await expect(
    reopened.getByRole('switch', {
      name: /Superintendent to project orchestrators/,
    }),
  ).toBeChecked()
  await expect(
    reopened.getByRole('switch', {
      name: /Project orchestrators to workers/,
    }),
  ).not.toBeChecked()
  await expect(
    reopened.getByRole('switch', {
      name: /Scheduled automatic summaries/,
    }),
  ).not.toBeChecked()

  await page.screenshot({
    path: testInfo.outputPath('token-spend-settings-desktop.png'),
    fullPage: true,
  })
  await page.setViewportSize({ width: 390, height: 844 })
  const manualCopy = reopened.getByText(
    'Prompts, routes, and Run now remain independently available.',
  )
  const manualBox = await manualCopy.boundingBox()
  const actionsBox = await reopened
    .locator('.token-spend-dialog__actions')
    .boundingBox()
  expect(manualBox).not.toBeNull()
  expect(actionsBox).not.toBeNull()
  expect(manualBox!.y + manualBox!.height).toBeLessThanOrEqual(actionsBox!.y)
  await page.screenshot({
    path: testInfo.outputPath('token-spend-settings-mobile.png'),
    fullPage: true,
  })
})

test('reloads edits and resets the server-persisted orchestrator workflow', async ({
  page,
}, testInfo) => {
  const state = await mockApi(page)
  await page.goto('/')

  const openWorkflow = async () => {
    const settings = await openSettings(page)
    await settings
      .getByRole('button', { name: 'Orchestrator workflow settings' })
      .click()
    return page.getByRole('dialog', { name: 'Orchestrator workflow' })
  }

  let dialog = await openWorkflow()
  const instructions = dialog.getByRole('textbox', {
    name: 'Workflow instructions (Markdown)',
  })
  const interval = dialog.getByRole('spinbutton', {
    name: 'Monitor interval (minutes)',
  })
  await expect(instructions).toHaveValue(factoryWorkflowInstructions)
  await expect(interval).toHaveValue('10')
  await expect(
    dialog.getByRole('button', { name: 'Save changes' }),
  ).toBeDisabled()
  await page.screenshot({
    path: testInfo.outputPath('orchestrator-workflow-desktop.png'),
    fullPage: true,
  })

  await instructions.fill('# Custom workflow\n\nUse three isolated lanes.')
  await interval.fill('15')
  await dialog.getByRole('button', { name: 'Save changes' }).click()
  await expect(dialog).toBeHidden()
  expect(state.orchestratorWorkflowUpdates).toEqual([
    {
      actor: 'local-user',
      expected_version: '1',
      instructions_markdown:
        '# Custom workflow\n\nUse three isolated lanes.',
      monitor_interval_ms: '900000',
    },
  ])

  await page.reload()
  dialog = await openWorkflow()
  await expect(
    dialog.getByRole('textbox', {
      name: 'Workflow instructions (Markdown)',
    }),
  ).toHaveValue('# Custom workflow\n\nUse three isolated lanes.')
  await expect(
    dialog.getByRole('spinbutton', {
      name: 'Monitor interval (minutes)',
    }),
  ).toHaveValue('15')

  await dialog.getByRole('button', { name: 'Reset to factory' }).click()
  expect(state.orchestratorWorkflowResets).toEqual([
    {
      actor: 'local-user',
      expected_version: '2',
    },
  ])
  await expect(
    dialog.getByRole('textbox', {
      name: 'Workflow instructions (Markdown)',
    }),
  ).toHaveValue(factoryWorkflowInstructions)
  await expect(dialog.getByText('Factory reset', { exact: true })).toBeVisible()

  await dialog
    .getByRole('button', { name: 'Close orchestrator workflow' })
    .click()
  await page.setViewportSize({ width: 390, height: 844 })
  await page.reload()
  dialog = await openWorkflow()
  await expect(
    dialog.getByRole('textbox', {
      name: 'Workflow instructions (Markdown)',
    }),
  ).toHaveValue(factoryWorkflowInstructions)
  await expect(
    dialog.getByRole('spinbutton', {
      name: 'Monitor interval (minutes)',
    }),
  ).toHaveValue('10')
  const bounds = await dialog.boundingBox()
  expect(bounds).not.toBeNull()
  expect(bounds!.x).toBeGreaterThanOrEqual(0)
  expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(390)
  await expect(
    dialog.getByRole('button', { name: 'Reset to factory' }),
  ).toBeVisible()
  await expect(
    dialog.getByRole('button', { name: 'Save changes' }),
  ).toBeVisible()
  await page.screenshot({
    path: testInfo.outputPath('orchestrator-workflow-mobile.png'),
    fullPage: true,
  })
})

test('edits workstream automation scope and distinguishes submission from completion', async ({
  page,
}, testInfo) => {
  const state = await mockApi(page)
  const now = Date.now()
  state.coordinationNodes.push({
    id: 'workstream-release',
    name: 'Release readiness',
    kind: 'workstream',
    placement: {
      geometry: { x: 420, y: 24, width: 116, height: 116 },
      version: '1',
      updated_at_unix_ms: now,
    },
    attached_project_ids: ['project-1', 'project-2'],
    worker: null,
    cwd: '/tmp/yard/coordination/release',
    folder_path: null,
    version: '1',
    created_by: 'local-user',
    created_at_unix_ms: now,
    updated_at_unix_ms: now,
  })
  const automation: Automation = {
    id: 'automation-daily',
    name: 'Portfolio check',
    scope: { kind: 'yard_orchestrator' },
    placement: {
      geometry: { x: 250, y: 18, width: 168, height: 58 },
      version: '1',
      updated_at_unix_ms: now,
    },
    schedule: { hour: 8, minute: 30, timezone: 'UTC' },
    selected_project_ids: ['project-1', 'project-2'],
    prompt_template: 'Review portfolio status.',
    state: 'active',
    next_run_at_unix_ms: now + 86_400_000,
    latest_run: null,
    version: '1',
    created_by: 'local-user',
    created_at_unix_ms: now,
    updated_at_unix_ms: now,
  }
  const history = Array.from({ length: 20 }, (_, index): AutomationRun => ({
    id: `historical-run-${index}`,
    automation_id: automation.id,
    automation_version: automation.version,
    trigger: index % 2 === 0 ? 'scheduled' : 'manual',
    scheduled_for_unix_ms: index % 2 === 0 ? now - index * 60_000 : null,
    dispatch_command_id: `dispatch-history-${index}`,
    prompt_template: automation.prompt_template,
    selected_project_ids: [...automation.selected_project_ids],
    status: index === 0 ? 'submitted' : 'pending',
    runtime_status: index === 0 ? 'accepted' : null,
    error_message: null,
    submitted_at_unix_ms: index === 0 ? now : null,
    requested_by: 'scheduler',
    version: '1',
    created_at_unix_ms: now - index * 60_000,
    updated_at_unix_ms: now - index * 60_000,
  }))
  automation.latest_run = history[0]
  state.automations.push(automation)
  state.automationRuns[automation.id] = history

  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')
  await page.locator('.automation-map-node').click()

  await expect(page.getByText('Automatic schedule', { exact: true })).toBeVisible()
  await expect(page.getByText('Manual dispatch', { exact: true })).toBeVisible()
  await expect(page.locator('.automation-run')).toHaveCount(20)
  const historyBounds = await page
    .locator('.automation-history__list')
    .evaluate((element) => ({
      clientHeight: element.clientHeight,
      scrollHeight: element.scrollHeight,
    }))
  expect(historyBounds.clientHeight).toBeLessThanOrEqual(320)
  expect(historyBounds.scrollHeight).toBeGreaterThan(
    historyBounds.clientHeight,
  )
  await expect(
    page.getByText('Transport accepted. Completion is not implied.'),
  ).toBeVisible()
  const projectedAutomationRoute = page.locator(
    '.projected-route[data-route-id="automation-target:automation-daily"]',
  )
  await expect(projectedAutomationRoute).toHaveAttribute(
    'data-state',
    'active',
  )
  await expect(
    projectedAutomationRoute.locator('.projected-route__activity'),
  ).toHaveCount(1)

  await page
    .getByLabel('Target orchestrator')
    .selectOption('workstream:workstream-release')
  await page
    .getByRole('checkbox', { name: 'Offline release' })
    .uncheck()
  await page.getByLabel('Name').fill('Release readiness check')
  await page.getByRole('button', { name: 'Save automation' }).click()
  expect(state.automationUpdateCommands).toHaveLength(1)
  expect(state.automationUpdateCommands[0]).toMatchObject({
    expected_version: '1',
    name: 'Release readiness check',
    scope: {
      kind: 'workstream_coordination_node',
      node_id: 'workstream-release',
    },
    selected_project_ids: ['project-1'],
  })

  await page.getByRole('button', { name: 'Pause schedule' }).click()
  expect(state.automationStateCommands[0]).toMatchObject({
    expected_version: '2',
    paused: true,
  })
  await expect(page.getByText('Paused', { exact: true })).toBeVisible()

  await page.getByRole('button', { name: 'Run now' }).click()
  expect(state.automationRunCommands[0]).toMatchObject({
    expected_version: '3',
  })
  await expect(
    page.getByText(
      'Automation run submitted to transport. Completion is not implied.',
    ),
  ).toBeVisible()
  await expect(page.locator('.automation-run')).toHaveCount(21)

  await page.setViewportSize({ width: 390, height: 844 })
  const inspectorOverflow = await page.locator('.inspector').evaluate(
    (element) => {
      const bounds = element.getBoundingClientRect()
      return {
        horizontal: element.scrollWidth - element.clientWidth,
        left: bounds.left,
        right: bounds.right - window.innerWidth,
      }
    },
  )
  expect(inspectorOverflow.horizontal).toBeLessThanOrEqual(0)
  expect(inspectorOverflow.left).toBeGreaterThanOrEqual(0)
  expect(inspectorOverflow.right).toBeLessThanOrEqual(0)
  await page.screenshot({
    path: testInfo.outputPath('automation-inspector-mobile.png'),
    fullPage: true,
  })
})

test('creates an attached knowledge store and requests a source-linked snapshot', async ({
  page,
}, testInfo) => {
  const state = await mockApi(page)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await expect(
    page.getByRole('button', { name: 'Create map node' }),
  ).toHaveCount(0)
  await page.locator('.react-flow__pane').click({
    button: 'right',
    position: { x: 520, y: 430 },
  })
  await page
    .getByRole('menuitem', { name: /Knowledge store/ })
    .click()
  await page.getByLabel('Name').fill('Platform knowledge')
  await page.getByRole('checkbox', { name: 'API migration' }).check()
  await page.getByRole('checkbox', { name: 'Offline release' }).check()
  await page
    .getByRole('button', { name: 'Create node', exact: true })
    .click()

  const node = page.locator(
    '.coordination-map-node[data-kind="knowledge_store"]',
  )
  await expect(node).toBeVisible()
  await expect(node).toContainText('Platform knowledge')
  await setMapView(page, '2D view')
  await setMapView(page, '2.5D view')
  const createdNodeId = state.coordinationNodes.at(-1)?.id
  expect(createdNodeId).toBeTruthy()
  const anchorSelector = `.projected-anchor--coordination-node[data-node-id="coordination-node:${createdNodeId}"] .projected-anchor__pad`
  const anchor = page.locator(
    anchorSelector,
  )
  await expect(anchor).toBeVisible()
  const grounding = await node
    .locator('.coordination-map-node__glyph')
    .evaluate((glyph, anchorSelector) => {
      const pad = document.querySelector<SVGElement>(anchorSelector)
      if (!pad) return null
      const glyphBounds = glyph.getBoundingClientRect()
      const padBounds = pad.getBoundingClientRect()
      return {
        horizontal:
          Math.abs(
            glyphBounds.left +
              glyphBounds.width / 2 -
              (padBounds.left + padBounds.width / 2),
          ),
        vertical: Math.abs(
          glyphBounds.bottom - (padBounds.top + padBounds.height / 2),
        ),
      }
    }, anchorSelector)
  expect(grounding).not.toBeNull()
  expect(grounding?.horizontal ?? Number.POSITIVE_INFINITY).toBeLessThanOrEqual(
    4,
  )
  expect(grounding?.vertical ?? Number.POSITIVE_INFINITY).toBeLessThanOrEqual(8)
  await expect(page.locator('.node-attachment-edge')).toHaveCount(2)
  expect(state.coordinationNodeCreateCommands).toHaveLength(1)
  expect(state.coordinationNodeCreateCommands[0]).toMatchObject({
    name: 'Platform knowledge',
    kind: 'knowledge_store',
    placement: {
      width: 116,
      height: 116,
    },
    attached_project_ids: ['project-1', 'project-2'],
  })

  await expect(
    page.getByText('Knowledge snapshots', { exact: true }),
  ).toBeVisible()
  await page
    .getByRole('button', { name: 'Request knowledge snapshot' })
    .click()
  await expect(page.getByText('0/2 collected')).toBeVisible()
  await expect(
    page.getByText('Knowledge collection requested from 2 projects.'),
  ).toBeVisible()
  expect(state.coordinationSnapshotCommands).toHaveLength(1)
  expect(state.coordinationSnapshots[0].projects).toHaveLength(2)
  expect(
    state.coordinationSnapshots[0].projects.every(
      (project) => project.delivery_status === 'submitted',
    ),
  ).toBe(true)

  await page.screenshot({
    path: testInfo.outputPath('knowledge-store.png'),
    fullPage: true,
  })
  await page.getByRole('button', { name: 'Close details' }).click()
  await page.getByRole('button', { name: 'Project pulse' }).click()
  await expect(page.getByLabel('Coordination nodes')).toContainText(
    'Platform knowledge',
  )
  await page
    .getByLabel('Coordination nodes')
    .getByRole('button', { name: /Platform knowledge/ })
    .click()
  await expect(
    page.getByRole('heading', { name: 'Platform knowledge' }),
  ).toBeVisible()
  await setAppTheme(page, 'Dark')
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark')
  await page.screenshot({
    path: testInfo.outputPath('knowledge-store-dark.png'),
    fullPage: true,
  })
  await page.setViewportSize({ width: 390, height: 844 })
  const inspectorBounds = await page.locator('.inspector').boundingBox()
  expect(inspectorBounds?.x ?? -1).toBeGreaterThanOrEqual(0)
  expect(
    inspectorBounds
      ? inspectorBounds.x + inspectorBounds.width
      : Number.POSITIVE_INFINITY,
  ).toBeLessThanOrEqual(390)
  await page.screenshot({
    path: testInfo.outputPath('knowledge-store-mobile.png'),
    fullPage: true,
  })
})

test('provisions a workstream orchestrator and routes work to an attached project', async ({
  page,
}) => {
  const reports: Record<string, unknown> = {}
  const state = await mockApi(page, { orchestratorStatusReports: reports })
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page.locator('.react-flow__pane').click({
    button: 'right',
    position: { x: 520, y: 430 },
  })
  await page
    .getByRole('menuitem', { name: /Workstream/ })
    .click()
  await page.getByLabel('Name').fill('Release readiness')
  await page.getByRole('checkbox', { name: 'API migration' }).check()
  await page
    .getByRole('button', { name: 'Create node', exact: true })
    .click()

  const node = page.locator(
    '.coordination-map-node[data-kind="workstream"]',
  )
  await expect(node).toHaveAttribute('data-provisioned', 'true')
  expect(state.coordinationNodeProvisionCommands).toHaveLength(1)
  expect(state.coordinationNodeProvisionCommands[0]).toMatchObject({
    profile_id: 'profile-1',
    expected_profile_version: '1',
    expected_node_version: '1',
  })

  await page
    .getByRole('button', { name: 'Open terminal', exact: true })
    .click()
  await expect(page.locator('.terminal-session')).toHaveAttribute(
    'data-state',
    'connected',
  )
  expect(new URL(state.terminalConnectionUrls[0]).pathname).toMatch(
    /^\/api\/v1\/coordination-nodes\/[^/]+\/terminal$/,
  )
  await page.getByRole('button', { name: 'Close terminal' }).click()

  await page
    .getByRole('button', { name: 'Open chat', exact: true })
    .click()
  await expect(page.getByLabel('Agent conversation')).toContainText(
    'Coordinating attached projects.',
  )
  await page.getByLabel('Dispatch scope').selectOption('project-1')
  await page
    .getByLabel('Message', { exact: true })
    .fill('Pick up the release-blocking migration work.')
  await page
    .getByRole('button', { name: 'Send order', exact: true })
    .click()
  await expect(
    page.getByText('Workstream delivered this order to API migration.'),
  ).toBeVisible()
  expect(state.coordinationNodeRouteCommands).toHaveLength(1)
  expect(state.coordinationNodeRouteCommands[0]).toMatchObject({
    target_project_id: 'project-1',
    target_orchestrator_worker_id: 'project-1-orchestrator',
  })
  await expect(
    page.locator('.node-attachment-edge.coordination-edge--active'),
  ).toHaveCount(1)

  const routeCommand = state.coordinationNodeRouteCommands[0]
  reports['project-1'] = {
    version: 1,
    command_id: routeCommand.command_id,
    state: 'idle',
    last: 'Shared the requested migration update.',
    next: 'Wait for more work.',
    blockers: [],
  }
  await page.reload()
  await expect(
    page.locator('.node-attachment-edge.coordination-edge--idle'),
  ).toHaveCount(1)
})

test('renders communication paths by durable activity state', async ({
  page,
}, testInfo) => {
  const state = await mockApi(page, {
    orchestratorStatusReports: {
      'project-3': {
        version: 1,
        command_id: 'route-complete',
        state: 'idle',
        last: 'Shared the requested project update.',
        next: 'Wait for more work.',
        blockers: [],
      },
    },
  })
  const now = Date.now()
  state.projects.push(
    project(
      'project-3',
      'Search indexing',
      'alpha',
      'workspace-3',
      'terminal-8',
      860,
    ),
  )
  state.projectRelationships.push({
    id: 'relationship-animated',
    source_project_id: 'project-1',
    target_project_id: 'project-2',
    kind: 'depends_on',
    version: '1',
    created_by: 'local-user',
    created_at_unix_ms: now,
    updated_at_unix_ms: now,
  })
  state.yardOrchestrator = {
    worker: durableWorker(
      'worker-yard-path',
      'terminal-2',
      state.profiles[0],
    ),
    version: '2',
    workflow_profile_version: '1',
    created_at_unix_ms: now,
    updated_at_unix_ms: now,
  }
  state.yardOrchestratorRoutes.push({
    command_id: 'route-failed',
    actor: 'local-user',
    orchestrator_worker_id: 'worker-yard-path',
    expected_orchestrator_version: '2',
    target_project_id: 'project-1',
    target_orchestrator_worker_id: 'project-1-orchestrator',
    expected_project_version: '1',
    text: 'Retry the blocked migration.',
    status: 'failed',
    error_message: 'Project orchestrator rejected the route.',
    runtime_status: null,
    created_at_unix_ms: now,
    updated_at_unix_ms: now,
    submitted_at_unix_ms: null,
  })
  state.yardOrchestratorRoutes.push(
    {
      command_id: 'route-active',
      actor: 'local-user',
      orchestrator_worker_id: 'worker-yard-path',
      expected_orchestrator_version: '2',
      target_project_id: 'project-2',
      target_orchestrator_worker_id: 'project-2-orchestrator',
      expected_project_version: '1',
      text: 'Share the current release state.',
      status: 'submitted',
      error_message: null,
      runtime_status: 'accepted',
      created_at_unix_ms: now,
      updated_at_unix_ms: now,
      submitted_at_unix_ms: now,
    },
    {
      command_id: 'route-complete',
      actor: 'local-user',
      orchestrator_worker_id: 'worker-yard-path',
      expected_orchestrator_version: '2',
      target_project_id: 'project-3',
      target_orchestrator_worker_id: 'project-3-orchestrator',
      expected_project_version: '1',
      text: 'Share the current indexing state.',
      status: 'submitted',
      error_message: null,
      runtime_status: 'accepted',
      created_at_unix_ms: now,
      updated_at_unix_ms: now,
      submitted_at_unix_ms: now,
    },
  )
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  const relationship = page.locator('.project-relationship-edge')
  const activeRoute = page.locator(
    '.coordination-edge.coordination-edge--active',
  )
  const failedRoute = page.locator(
    '.coordination-edge.coordination-edge--failed',
  )
  const idleRoute = page.locator(
    '.coordination-edge.coordination-edge--idle',
  )
  await expect(activeRoute).toHaveCount(1)
  await expect(failedRoute).toHaveCount(1)
  await expect(idleRoute).toHaveCount(1)
  await expect(relationship).not.toHaveClass(/animated/)
  await expect(activeRoute).toHaveClass(/animated/)
  await expect(failedRoute).not.toHaveClass(/animated/)
  await expect(idleRoute).not.toHaveClass(/animated/)
  await expect(activeRoute).toHaveCSS('visibility', 'hidden')
  expect(
    await activeRoute
      .locator('.react-flow__edge-path')
      .evaluate((element) => getComputedStyle(element).animationName),
  ).toBe('none')
  await expect(activeRoute.locator('.react-flow__edge-path')).toHaveCSS(
    'filter',
    'none',
  )
  await expect(activeRoute).toHaveCSS('opacity', '0.8')
  await expect(activeRoute.locator('.react-flow__edge-path')).toHaveCSS(
    'stroke',
    'rgb(25, 118, 107)',
  )
  expect(
    await failedRoute
      .locator('.react-flow__edge-path')
      .evaluate((element) => getComputedStyle(element).stroke),
  ).toBe('rgb(217, 74, 55)')
  await expect(idleRoute).toHaveCSS('opacity', '0.8')
  await expect(idleRoute.locator('.react-flow__edge-path')).toHaveCSS(
    'stroke',
    'rgb(104, 115, 111)',
  )
  await expect(relationship).toHaveCSS('opacity', '0.8')

  // The rail infrastructure stays neutral. Signals carry route state, and only
  // active communication gets a moving train packet.
  const projectedActive = page
    .locator('.projected-route--coordination.projected-route--active')
    .first()
  const projectedFailed = page
    .locator('.projected-route--coordination.projected-route--failed')
    .first()
  const projectedIdle = page
    .locator('.projected-route--coordination.projected-route--idle')
    .first()
  const activeSignal = projectedActive.locator(
    '.projected-route__signal-light',
  )
  const failedSignal = projectedFailed.locator(
    '.projected-route__signal-light',
  )
  const idleSignal = projectedIdle.locator('.projected-route__signal-light')
  await expect(activeSignal).toHaveCSS('fill', 'rgb(25, 118, 107)')
  await expect(failedSignal).toHaveCSS('fill', 'rgb(217, 74, 55)')
  await expect(idleSignal).toHaveCSS('fill', 'rgb(104, 115, 111)')
  await expect(projectedIdle).toHaveCSS('opacity', '0.8')
  const activePacket = projectedActive.locator('.projected-route__activity')
  await expect(activePacket).toHaveCount(1)
  await expect(activePacket).toHaveCSS('stroke', 'rgb(25, 118, 107)')
  await expect(
    projectedFailed.locator('.projected-route__activity'),
  ).toHaveCount(0)
  await expect(
    projectedIdle.locator('.projected-route__activity'),
  ).toHaveCount(0)
  expect(
    await activePacket
      .evaluate((element) => getComputedStyle(element).animationName),
  ).toBe('projected-train-run')
  expect(
    await activePacket.evaluate((element) =>
      Number.parseFloat(getComputedStyle(element).animationDuration),
    ),
  ).toBe(0.7)
  for (const layer of ['ballast', 'sleepers', 'rails', 'gauge']) {
    const strokes = await Promise.all(
      [projectedActive, projectedFailed, projectedIdle].map((route) =>
        route
          .locator(`.projected-route__${layer}`)
          .evaluate((element) => getComputedStyle(element).stroke),
      ),
    )
    expect(new Set(strokes).size).toBe(1)
  }
  await page.screenshot({
    path: testInfo.outputPath('railroad-light.png'),
    fullPage: true,
  })
  const railroadSettings = await openSettings(page)
  await railroadSettings
    .getByRole('button', { name: 'Dark', exact: true })
    .click()
  await railroadSettings
    .getByRole('button', { name: 'Close settings' })
    .click()
  await page.screenshot({
    path: testInfo.outputPath('railroad-dark.png'),
    fullPage: true,
  })

  await setMapView(page, '2D view')
  await expect(page.locator('.projected-route')).toHaveCount(0)
  await expect(activeRoute).toHaveCSS('visibility', 'visible')
  const twoDPulse = activeRoute.locator('.react-flow__edge-path')
  expect(
    await twoDPulse.evaluate(
      (element) => getComputedStyle(element).animationName,
    ),
  ).toBe('information-path-flow')
  expect(
    await twoDPulse.evaluate((element) =>
      Number.parseFloat(getComputedStyle(element).animationDuration),
    ),
  ).toBe(0.75)

  await page.emulateMedia({ reducedMotion: 'reduce' })
  expect(
    await twoDPulse
      .evaluate((element) => getComputedStyle(element).animationName),
  ).toBe('none')
  await setMapView(page, '2.5D view')
  expect(
    await page
      .locator('.projected-route--coordination.projected-route--active')
      .first()
      .locator('.projected-route__activity')
      .evaluate((element) => getComputedStyle(element).animationName),
  ).toBe('none')
  await expect(
    relationship.locator('.react-flow__edge-path'),
  ).toHaveCSS('stroke-dasharray', '6px, 5px')
})

test('renders the map as one projected world and persists the view mode', async ({
  page,
}, testInfo) => {
  await mockApi(page)
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto('/')

  const canvas = page.getByLabel('Yard project canvas')
  const projectNode = page.locator('.react-flow__node-project').first()
  const projectRegion = projectNode.locator('.project-region')
  const minimap = page.locator('.react-flow__minimap')

  await expect(canvas).toHaveAttribute('data-visual-mode', 'depth')
  const settings = await openSettings(page)
  await expect(
    settings.getByRole('button', { name: '2.5D view' }),
  ).toHaveAttribute('aria-pressed', 'true')
  await expect(
    settings.getByRole('button', { name: '2D view' }),
  ).toHaveAttribute('aria-pressed', 'false')
  await settings.getByRole('button', { name: 'Close settings' }).click()
  await expect(minimap).toBeVisible()
  expect(
    await canvas.evaluate(
      (element) => getComputedStyle(element).backgroundImage,
    ),
  ).not.toBe('none')

  // The territory is a projected parallelogram, not an axis-aligned card. The
  // previous version of this test asserted the opposite — that toggling 2.5D
  // left the flat node box untouched — which is exactly the decorative
  // behaviour this slice replaces, so that assertion is gone on purpose.
  const territory = page.locator('.territory-polygon').first()
  await expect(territory).toHaveCount(1)
  const corners = await territory.evaluate((element) =>
    (element.getAttribute('points') ?? '')
      .trim()
      .split(/\s+/)
      .map((pair) => {
        const [x, y] = pair.split(',').map(Number)
        return { x, y }
      }),
  )
  expect(corners).toHaveLength(4)
  for (let index = 0; index < corners.length; index += 1) {
    const from = corners[index]
    const to = corners[(index + 1) % corners.length]
    expect(Math.abs(to.x - from.x)).toBeGreaterThan(1)
    expect(Math.abs(to.y - from.y)).toBeGreaterThan(1)
  }

  // Structures stand on the territory they belong to: every building face sits
  // inside the territory's own projected envelope, widened only by the height
  // the structures rise.
  const buildingsInsideTerritory = await page
    .locator('.projected-territory')
    .first()
    .evaluate((group) => {
      const outline = group.querySelector('.territory-polygon')
      if (!outline) return null
      const bounds = (outline as SVGGraphicsElement).getBBox()
      const faces = Array.from(
        group.querySelectorAll<SVGGraphicsElement>(
          '.projected-building__face--top, .projected-building__face--side, .projected-building__face--side-b',
        ),
      )
      return {
        count: faces.length,
        outside: faces.filter((face) => {
          const box = face.getBBox()
          return (
            box.x < bounds.x - 1 ||
            box.x + box.width > bounds.x + bounds.width + 1 ||
            box.y + box.height > bounds.y + bounds.height + 1
          )
        }).length,
      }
    })
  expect(buildingsInsideTerritory).not.toBeNull()
  expect(buildingsInsideTerritory!.count).toBeGreaterThan(0)
  expect(buildingsInsideTerritory!.outside).toBe(0)

  await setMapView(page, '2D view')
  await expect(canvas).toHaveAttribute('data-visual-mode', 'flat')
  expect(
    await page.evaluate(() =>
      window.localStorage.getItem('yard:map-visual-mode:v1'),
    ),
  ).toBe('flat')
  await expect(page.locator('.territory-polygon')).toHaveCount(0)

  await setMapView(page, '2.5D view')
  await expect(canvas).toHaveAttribute('data-visual-mode', 'depth')
  expect(
    await page.evaluate(() =>
      window.localStorage.getItem('yard:map-visual-mode:v1'),
    ),
  ).toBe('depth')
  await page.reload()
  await expect(canvas).toHaveAttribute('data-visual-mode', 'depth')

  // Dispatch selection on the ReactFlow node itself so this z-order assertion
  // does not depend on whichever upright unit occupies a particular pixel.
  await projectNode.dispatchEvent('click')
  await expect(projectRegion).toHaveClass(/is-selected/)
  await expect(page.locator('.projected-territory').first()).toHaveClass(
    /is-selected/,
  )
  const depthLayers = await page.evaluate(() => {
    const project = document.querySelector<HTMLElement>(
      '.react-flow__node-project.selected',
    )
    const units = Array.from(
      document.querySelectorAll<HTMLElement>(
        [
          '.react-flow__node-worker',
          '.react-flow__node-orchestrator',
          '.react-flow__node-assigned-worker',
          '.react-flow__node-child-agent',
          '.react-flow__node-yard-orchestrator',
          '.react-flow__node-automation',
          '.react-flow__node-coordination-node',
        ].join(', '),
      ),
    )
    return {
      project: project ? Number(getComputedStyle(project).zIndex) : -1,
      units: units.map((unit) => Number(getComputedStyle(unit).zIndex)),
    }
  })
  expect(depthLayers.units.length).toBeGreaterThan(0)
  expect(
    depthLayers.units.every((zIndex) => zIndex > depthLayers.project),
  ).toBe(true)
  const lightGround = await page
    .locator('.projected-ground')
    .evaluate((element) => getComputedStyle(element).fill)
  await page.screenshot({
    path: testInfo.outputPath('raised-map-desktop.png'),
    fullPage: true,
  })

  // Forced colours must not swallow the selection outline; it is the only cue
  // that survives when the accent palette is replaced.
  await page.emulateMedia({ forcedColors: 'active' })
  const forcedOutline = await page
    .locator('.projected-territory.is-selected .territory-polygon__outline')
    .evaluate((element) => {
      const style = getComputedStyle(element)
      return { stroke: style.stroke, width: style.strokeWidth }
    })
  expect(forcedOutline.stroke).not.toBe('none')
  expect(Number.parseFloat(forcedOutline.width)).toBeGreaterThanOrEqual(3)
  await page.emulateMedia({ forcedColors: 'none' })

  await setAppTheme(page, 'Dark')
  // The projected surface is theme aware: it reads the same tokens the rest of
  // the shell does rather than baking in a light palette.
  await expect
    .poll(async () =>
      page
        .locator('.projected-ground')
        .evaluate((element) => getComputedStyle(element).fill),
    )
    .not.toBe(lightGround)
  await page.screenshot({
    path: testInfo.outputPath('raised-map-dark.png'),
    fullPage: true,
  })

  await page.setViewportSize({ width: 390, height: 844 })
  await page.reload()
  await expect(canvas).toHaveAttribute('data-visual-mode', 'depth')
  const toolsBounds = await page.locator('.canvas-tools-panel').boundingBox()
  expect(toolsBounds).not.toBeNull()
  expect(toolsBounds!.x).toBeGreaterThanOrEqual(0)
  expect(toolsBounds!.x + toolsBounds!.width).toBeLessThanOrEqual(390)
  await page.screenshot({
    path: testInfo.outputPath('raised-map-mobile.png'),
    fullPage: true,
  })
})

test('arranges and persists non-overlapping project spaces around Yard', async ({
  page,
}, testInfo) => {
  const state = await mockApi(page)
  state.projects.push(
    project(
      'project-3',
      'Search indexing',
      'alpha',
      'workspace-3',
      'terminal-8',
      220,
    ),
    project(
      'project-4',
      'Docs refresh',
      'alpha',
      'workspace-4',
      'terminal-9',
      300,
    ),
  )
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto('/')

  const yardNode = page.locator('[data-id="yard-orchestrator"]')
  const yardPosition = await yardNode.evaluate(
    (element) => (element as HTMLElement).style.transform,
  )
  await page.getByRole('button', { name: 'Arrange spaces' }).click()
  await expect
    .poll(() => state.placementUpdates, { timeout: 5_000 })
    .toBe(state.projects.length)
  expect(
    await yardNode.evaluate(
      (element) => (element as HTMLElement).style.transform,
    ),
  ).toBe(yardPosition)

  for (let index = 0; index < state.projects.length; index += 1) {
    const current = state.projects[index].placement.geometry
    for (
      let otherIndex = index + 1;
      otherIndex < state.projects.length;
      otherIndex += 1
    ) {
      const other = state.projects[otherIndex].placement.geometry
      const overlaps =
        current.x < other.x + other.width &&
        current.x + current.width > other.x &&
        current.y < other.y + other.height &&
        current.y + current.height > other.y
      expect(overlaps).toBe(false)
    }
  }

  const distances = await page
    .locator(
      '[data-id="yard-orchestrator"], .react-flow__node-project',
    )
    .evaluateAll((elements) => {
      const centers = elements.map((element) => {
        const matrix = new DOMMatrix(getComputedStyle(element).transform)
        return {
          id: element.getAttribute('data-id'),
          x: matrix.e + (element as HTMLElement).offsetWidth / 2,
          y: matrix.f + (element as HTMLElement).offsetHeight / 2,
        }
      })
      const yard = centers.find((center) => center.id === 'yard-orchestrator')
      if (!yard) return []
      return centers
        .filter((center) => center.id?.startsWith('project:'))
        .map((center) => Math.hypot(center.x - yard.x, center.y - yard.y))
    })
  expect(distances).toHaveLength(state.projects.length)
  expect(Math.max(...distances) - Math.min(...distances)).toBeLessThan(2)

  await expect
    .poll(async () => {
      const canvas = await page.locator('.canvas-stage').boundingBox()
      const projectBounds = await page
        .locator('.react-flow__node-project')
        .evaluateAll((elements) =>
          elements.map((element) => {
            const bounds = element.getBoundingClientRect()
            return {
              bottom: bounds.bottom,
              left: bounds.left,
              right: bounds.right,
              top: bounds.top,
            }
          }),
        )
      return Boolean(
        canvas &&
          projectBounds.every(
            (bounds) =>
              bounds.left >= canvas.x &&
              bounds.top >= canvas.y &&
              bounds.right <= canvas.x + canvas.width &&
              bounds.bottom <= canvas.y + canvas.height,
          ),
      )
    })
    .toBe(true)

  // Arrange frames the projected envelope, not ReactFlow's flat node boxes:
  // every visible territory has to land inside the canvas, and those boxes no
  // longer agree with what is drawn.
  await expect
    .poll(async () =>
      page.evaluate(() => {
        const canvas = document
          .querySelector('.runtime-canvas')!
          .getBoundingClientRect()
        return Array.from(
          document.querySelectorAll(
            '.projected-territory--project .territory-polygon',
          ),
        ).every((polygon) => {
          const box = polygon.getBoundingClientRect()
          return (
            box.left >= canvas.x - 1 &&
            box.top >= canvas.y - 1 &&
            box.right <= canvas.x + canvas.width + 1 &&
            box.bottom <= canvas.y + canvas.height + 1
          )
        })
      }),
    )
    .toBe(true)

  const persisted = state.projects.map((current) => ({
    id: current.id,
    placement: { ...current.placement.geometry },
  }))
  await page.screenshot({
    path: testInfo.outputPath('arranged-project-ring.png'),
    fullPage: true,
  })
  await page.reload()
  await expect(page.locator('.react-flow__node-project')).toHaveCount(
    state.projects.length,
  )
  expect(
    state.projects.map((current) => ({
      id: current.id,
      placement: current.placement.geometry,
    })),
  ).toEqual(persisted)
})

test('connects projects with a durable dependency edge', async ({
  page,
}) => {
  const state = await mockApi(page)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  const sourceHandle = page.locator(
    '[data-id="project:project-1"] .project-relationship-handle.source',
  )
  const targetHandle = page.locator(
    '[data-id="project:project-2"] .project-relationship-handle.target',
  )
  const sourceBounds = await sourceHandle.boundingBox()
  const targetBounds = await targetHandle.boundingBox()
  expect(sourceBounds).not.toBeNull()
  expect(targetBounds).not.toBeNull()
  await page.mouse.move(
    sourceBounds!.x + sourceBounds!.width / 2,
    sourceBounds!.y + sourceBounds!.height / 2,
  )
  await page.mouse.down()
  await page.mouse.move(
    targetBounds!.x + targetBounds!.width / 2,
    targetBounds!.y + targetBounds!.height / 2,
    { steps: 8 },
  )
  await page.mouse.up()

  await expect.poll(() => state.projectRelationships.length).toBe(1)
  expect(state.projectRelationships[0]).toMatchObject({
    source_project_id: 'project-1',
    target_project_id: 'project-2',
    kind: 'depends_on',
  })
  await expect(page.locator('.project-relationship-edge')).toHaveCount(1)

  // The relationship is a layered railroad whose right-angle segments run
  // along the ground plane's own axes.
  const routeShape = await page
    .locator('.projected-route--relationship')
    .first()
    .evaluate((group) => {
      const rails = group.querySelector('.projected-route__rails')
      const dots = Array.from(
        group.querySelectorAll('.projected-route__junction'),
      )
      const commands =
        (rails?.getAttribute('d') ?? '').match(/[ML][^ML]*/g) ?? []
      const points = commands.map((command) => {
        const [x, y] = command.slice(1).trim().split(' ').map(Number)
        return { x, y }
      })
      return {
        layerCount: group.querySelectorAll(
          '.projected-route__ballast, .projected-route__sleepers, .projected-route__rails, .projected-route__gauge',
        ).length,
        signalCount: group.querySelectorAll('.projected-route__signal').length,
        dots: dots.map((dot) => ({
          x: Number(dot.getAttribute('cx')),
          y: Number(dot.getAttribute('cy')),
        })),
        points,
      }
    })
  expect(routeShape.points.length).toBeGreaterThanOrEqual(2)
  expect(routeShape.layerCount).toBe(4)
  expect(routeShape.signalCount).toBe(1)
  expect(routeShape.dots).toEqual(routeShape.points)
  // Each leg runs along a projected ground axis, so its screen slope matches
  // one of the two axis gradients (0.44 / 0.82) rather than an arbitrary curve.
  for (let index = 1; index < routeShape.points.length; index += 1) {
    const from = routeShape.points[index - 1]
    const to = routeShape.points[index]
    const slope = Math.abs((to.y - from.y) / (to.x - from.x))
    expect(slope).toBeCloseTo(0.44 / 0.82, 3)
  }

  await page
    .locator('[data-id="project:project-1"]')
    .dispatchEvent('click')
  await expect(page.getByLabel('Project connections')).toContainText(
    'Offline release',
  )
  await page
    .getByRole('button', {
      name: 'Remove connection with Offline release',
    })
    .click()
  await expect.poll(() => state.projectRelationships.length).toBe(0)
  await expect(page.locator('.project-relationship-edge')).toHaveCount(0)
})

test('controls the project orchestrator terminal, output, and prompt idempotently', async ({
  page,
}) => {
  const state = await mockApi(page, {
    promptLosesResponseOnce: true,
    terminalOutputText: 'Orchestrator is coordinating two active workers.',
  })
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page
    .locator('.orchestrator-marker[data-project-id="project-1"]')
    .click()
  await expect(
    page.getByRole('button', { name: 'Open chat', exact: true }),
  ).toBeVisible()
  await page
    .getByRole('button', { name: 'Open terminal', exact: true })
    .click()
  const terminal = page.getByLabel('Interactive orchestrator terminal')
  await expect(terminal).toBeVisible()
  await expect.poll(() => state.terminalSockets.length).toBe(1)
  const terminalUrl = new URL(state.terminalConnectionUrls[0])
  expect(terminalUrl.pathname).toBe(
    '/api/v1/projects/project-1/orchestrator/terminal',
  )
  await page.getByRole('button', { name: 'Close terminal' }).click()
  await expect
    .poll(() =>
      state.terminalMessages.some(
        (message) => message.type === 'terminal.release',
      ),
    )
    .toBe(true)

  await page
    .getByRole('button', { name: 'Open chat', exact: true })
    .click()
  await expect(page.getByLabel('Agent conversation')).toContainText(
    'Orchestrator is coordinating two active workers.',
  )
  expect(
    state.orchestratorTerminalOutputRequests.filter(
      (request) => request.lines === '160',
    ),
  ).toEqual([{ projectId: 'project-1', lines: '160' }])

  const prompt = page.getByLabel('Message', { exact: true })
  const submit = page.getByRole('button', {
    name: 'Send order',
    exact: true,
  })
  await prompt.fill('Rebalance attention toward the blocked worker.')
  await submit.click()
  await expect(
    page.getByText('Herdr acknowledgement unavailable.'),
  ).toBeVisible()
  const retainedCommandId =
    state.orchestratorPromptRequestCommandIds[0]
  await submit.click()
  await expect(
    page.getByText('Order delivered to the agent.'),
  ).toBeVisible()

  expect(state.orchestratorPromptCommands[0]).toMatchObject({
    actor: 'local-user',
    expected_project_version: '1',
    orchestrator_worker_id: 'project-1-orchestrator',
  })
  expect(state.orchestratorPromptCommands[0].text).toContain(
    'Rebalance attention toward the blocked worker.',
  )
  expect(state.orchestratorPromptCommands[0].text).toContain(
    'MANUAL INTERVENTION',
  )
  expect(state.orchestratorPromptRequestCommandIds).toEqual([
    retainedCommandId,
    retainedCommandId,
  ])
  expect(state.orchestratorPromptDeliveries).toHaveLength(1)
  expect(state.orchestratorPromptReplayCommandIds).toEqual([
    retainedCommandId,
  ])

  await page.setViewportSize({ width: 390, height: 844 })
})

test('changes a project orchestrator only to an eligible live workspace worker', async ({
  page,
}, testInfo) => {
  const state = await mockApi(page)
  state.projects[0].name = 'Yard'
  const eligibleIndex = state.workerCandidates.findIndex(
    (candidate) => candidate.worker.id === 'worker-unassigned',
  )
  state.workerCandidates[eligibleIndex] = {
    ...state.workerCandidates[eligibleIndex],
    profile_name: 'Ready worker',
  }
  const originalProject = structuredClone(state.projects[0])
  const eligibleWorker = structuredClone(
    state.workerCandidates[eligibleIndex].worker,
  )
  const wrongWorkspaceWorker = durableWorker(
    'worker-wrong-workspace',
    'terminal-7',
    state.profiles[0],
    'workspace-2',
  )
  const wrongSessionWorker = durableWorker(
    'worker-wrong-session',
    'terminal-beta',
    state.profiles[0],
    'workspace-1',
    'beta',
  )
  const wrongAdapterWorker = durableWorker(
    'worker-wrong-adapter',
    'terminal-other-adapter',
    state.profiles[0],
  )
  wrongAdapterWorker.runtime = {
    ...wrongAdapterWorker.runtime!,
    adapter: 'other',
  }
  const unreadyWorker = durableWorker(
    'worker-not-interactive',
    'terminal-4',
    state.profiles[0],
  )
  state.runtimeInventory.workers = state.runtimeInventory.workers.map(
    (observed) =>
      observed.terminal_id === 'terminal-4'
        ? { ...observed, interactive_ready: false }
        : observed,
  )
  state.workerCandidates.push(
    {
      worker: wrongWorkspaceWorker,
      profile_name: 'Wrong workspace',
      availability: 'unassigned_live',
    },
    {
      worker: wrongSessionWorker,
      profile_name: 'Wrong session',
      availability: 'unassigned_live',
    },
    {
      worker: wrongAdapterWorker,
      profile_name: 'Wrong adapter',
      availability: 'unassigned_live',
    },
    {
      worker: unreadyWorker,
      profile_name: 'Not interactive',
      availability: 'unassigned_live',
    },
  )
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page
    .locator('.orchestrator-marker[data-project-id="project-1"]')
    .click()
  const inspector = page.locator('.inspector')
  await inspector
    .getByRole('button', { name: 'Change orchestrator' })
    .click()
  const dialog = page.getByRole('dialog', { name: 'Change orchestrator' })
  const workerSelect = dialog.getByLabel('Next orchestrator')
  await expect(workerSelect.locator('option')).toHaveCount(1)
  await expect(workerSelect).toHaveValue('worker-unassigned')
  await expect(dialog.locator('.ownership-transfer-impact')).toContainText(
    'Ready worker takes project orchestration for Yard',
  )
  await expect(dialog.locator('.ownership-transfer-impact')).toContainText(
    'project-1-orchestrator) remains live and becomes unassigned',
  )
  await page.screenshot({
    path: testInfo.outputPath('project-orchestrator-transfer-desktop.png'),
    fullPage: true,
  })

  const requestCounts = {
    inventory: state.inventoryRequests,
    projects: state.projectRequests,
    workers: state.workerRequests,
  }
  await dialog
    .getByRole('button', { name: 'Change orchestrator' })
    .click()
  await expect.poll(() => state.projectOrchestratorCommands.length).toBe(1)
  expect(state.projectOrchestratorCommands[0]).toMatchObject({
    actor: 'local-user',
    worker_id: eligibleWorker.id,
    expected_worker_version: eligibleWorker.version,
    expected_worker_runtime: eligibleWorker.runtime,
    expected_project_version: originalProject.version,
    expected_orchestrator_worker_id: originalProject.orchestrator.id,
    expected_orchestrator_worker_version:
      originalProject.orchestrator.version,
    expected_orchestrator_runtime: originalProject.orchestrator.runtime,
  })
  await expect(dialog).toHaveCount(0)
  await expect(
    inspector
      .getByRole('region', { name: 'Orchestrator runtime' })
      .locator('.detail-row')
      .filter({ hasText: 'Worker ID' }),
  ).toContainText('worker-unassigned')
  await expect(inspector.getByRole('heading', { name: 'Yard' })).toBeVisible()
  expect(state.projectRequests).toBeGreaterThan(requestCounts.projects)
  expect(state.workerRequests).toBeGreaterThan(requestCounts.workers)
  expect(state.inventoryRequests).toBeGreaterThan(requestCounts.inventory)

  const replacedCandidate = state.workerCandidates.find(
    (candidate) => candidate.worker.id === 'project-1-orchestrator',
  )
  expect(replacedCandidate).toMatchObject({
    availability: 'unassigned_live',
    project_id: null,
  })
  expect(replacedCandidate?.worker.runtime).toEqual(
    originalProject.orchestrator.runtime,
  )

  await inspector
    .getByRole('button', { name: 'Open terminal', exact: true })
    .click()
  await expect(
    page.locator('.agent-workspace-toolbar__target small'),
  ).toHaveText('terminal-2')
  await expect.poll(() => state.terminalSockets.length).toBe(1)
  await page.getByRole('button', { name: 'Close terminal' }).click()
  await page.getByRole('tab', { name: 'Workers' }).click()
  await expect(
    page.locator(
      '.worker-row[data-worker-id="project-1-orchestrator"][data-availability="unassigned_live"]',
    ),
  ).toBeVisible()
  await expect(
    page.locator(
      '.worker-row[data-worker-id="worker-unassigned"][data-availability="orchestrator"]',
    ),
  ).toBeVisible()
})

test('loads transfer inventory from the project session without changing the global session', async ({
  page,
}) => {
  const state = await mockApi(page)
  moveProjectTransferFixtureToSession(state, 'beta')

  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')
  state.requestLog = []
  await page
    .locator('.orchestrator-marker[data-project-id="project-1"]')
    .click()

  const inspector = page.locator('.inspector')
  const transfer = inspector.getByRole('button', {
    name: 'Change orchestrator',
  })
  await expect(transfer).toBeEnabled()
  await expect
    .poll(() => state.inventoryRequestSessions.includes('beta'))
    .toBe(true)
  const betaInventoryComplete = state.requestLog.findIndex((entry) =>
    entry.startsWith('inventory:beta:complete:'),
  )
  expect(betaInventoryComplete).toBeGreaterThanOrEqual(0)
  expect(
    state.requestLog.findIndex(
      (entry, index) => index > betaInventoryComplete && entry === 'workers',
    ),
  ).toBeGreaterThan(betaInventoryComplete)
  expect(
    state.requestLog.findIndex(
      (entry, index) =>
        index > betaInventoryComplete && entry === 'project:project-1',
    ),
  ).toBeGreaterThan(betaInventoryComplete)

  const runtimeHealth = await openRuntimeHealth(page)
  await expect(runtimeHealth.getByLabel('Herdr session')).toHaveValue('alpha')
  await page.keyboard.press('Escape')

  await transfer.click()
  const dialog = page.getByRole('dialog', {
    name: 'Change orchestrator',
  })
  await expect(dialog.getByLabel('Next orchestrator')).toHaveValue(
    'worker-unassigned',
  )
  await dialog
    .getByRole('button', { name: 'Change orchestrator' })
    .click()
  await expect.poll(() => state.projectOrchestratorCommands.length).toBe(1)
  expect(
    state.projectOrchestratorCommands[0].expected_worker_runtime.session,
  ).toBe('beta')
  expect(
    state.projectOrchestratorCommands[0].expected_orchestrator_runtime.session,
  ).toBe('beta')
})

test('submits full runtimes from a timestamp-reconciled transfer snapshot', async ({
  page,
}) => {
  const state = await mockApi(page)
  moveProjectTransferFixtureToSession(state, 'beta')
  const initialObservedAt =
    state.runtimeInventory.observed_at_unix_ms + 1_000
  const initialGate = deferred()
  state.inventoryResponsePlans.set(
    'beta',
    Array.from({ length: 4 }, () => ({
      observedAtUnixMs: initialObservedAt,
      reconcileRuntimeTimestamps: true,
      wait: initialGate.promise,
    })),
  )

  await page.goto('/')
  state.requestLog = []
  await page
    .locator('.orchestrator-marker[data-project-id="project-1"]')
    .click()
  const inspector = page.locator('.inspector')
  const transfer = inspector.getByRole('button', {
    name: 'Change orchestrator',
  })
  await expect
    .poll(() =>
      state.requestLog.some((entry) =>
        entry.startsWith('inventory:beta:start:'),
      ),
    )
    .toBe(true)
  await expect(transfer).toBeDisabled()
  await expect(inspector.getByRole('status')).toContainText(
    'Checking live workers in Herdr session beta',
  )

  initialGate.resolve()
  await expect(transfer).toBeEnabled()
  const initialCompleteIndex = state.requestLog.findIndex(
    (entry) =>
      entry === `inventory:beta:complete:${initialObservedAt}`,
  )
  expect(initialCompleteIndex).toBeGreaterThanOrEqual(0)
  expect(
    state.requestLog.findIndex(
      (entry, index) => index > initialCompleteIndex && entry === 'workers',
    ),
  ).toBeGreaterThan(initialCompleteIndex)
  expect(
    state.requestLog.findIndex(
      (entry, index) =>
        index > initialCompleteIndex && entry === 'project:project-1',
    ),
  ).toBeGreaterThan(initialCompleteIndex)

  const dialogRefreshGate = deferred()
  state.requestLog = []
  state.inventoryResponsePlans.set(
    'beta',
    Array.from({ length: 4 }, () => ({
      observedAtUnixMs: initialObservedAt,
      reconcileRuntimeTimestamps: true,
      wait: dialogRefreshGate.promise,
    })),
  )
  await transfer.click()
  const dialog = page.getByRole('dialog', {
    name: 'Change orchestrator',
  })
  const confirm = dialog.getByRole('button', {
    name: 'Change orchestrator',
  })
  await expect
    .poll(() =>
      state.requestLog.includes(
        `inventory:beta:start:${initialObservedAt}`,
      ),
    )
    .toBe(true)
  await expect(confirm).toBeDisabled()
  dialogRefreshGate.resolve()
  await expect(dialog.getByLabel('Next orchestrator')).toHaveValue(
    'worker-unassigned',
  )
  await expect(confirm).toBeEnabled()

  const submitObservedAt = initialObservedAt + 1_000
  const submitGate = deferred()
  state.requestLog = []
  state.inventoryResponsePlans.set('beta', [
    {
      observedAtUnixMs: submitObservedAt,
      reconcileRuntimeTimestamps: true,
      wait: submitGate.promise,
    },
  ])
  await confirm.click()
  await expect(confirm).toBeDisabled()
  await expect
    .poll(() =>
      state.requestLog.includes(
        `inventory:beta:start:${submitObservedAt}`,
      ),
    )
    .toBe(true)
  expect(state.projectOrchestratorCommands).toHaveLength(0)
  const reconciledProject = structuredClone(state.projects[0])
  const reconciledCandidate = structuredClone(
    state.workerCandidates.find(
      ({ worker: candidateWorker }) =>
        candidateWorker.id === 'worker-unassigned',
    ),
  )
  if (!reconciledCandidate?.worker.runtime) {
    throw new Error('Reconciled transfer candidate is missing')
  }

  submitGate.resolve()
  await expect
    .poll(() => state.projectOrchestratorCommands.length)
    .toBe(1)
  const command = state.projectOrchestratorCommands[0]
  expect(command.expected_project_version).toBe(
    reconciledProject.version,
  )
  expect(command.expected_worker_version).toBe(
    reconciledCandidate.worker.version,
  )
  expect(command.expected_worker_runtime).toEqual(
    reconciledCandidate.worker.runtime,
  )
  expect(command.expected_orchestrator_worker_version).toBe(
    reconciledProject.orchestrator.version,
  )
  expect(command.expected_orchestrator_runtime).toEqual(
    reconciledProject.orchestrator.runtime,
  )
  expect(
    command.expected_worker_runtime.last_observed_at_unix_ms,
  ).toBe(submitObservedAt)
  expect(
    command.expected_orchestrator_runtime.last_observed_at_unix_ms,
  ).toBe(submitObservedAt)
})

test('keeps transfer dialog focus contained during initial and polling refreshes', async ({
  page,
}) => {
  const state = await mockApi(page)
  moveProjectTransferFixtureToSession(state, 'beta')

  await page.goto('/')
  await page
    .locator('.orchestrator-marker[data-project-id="project-1"]')
    .click()
  const transfer = page
    .locator('.inspector')
    .getByRole('button', { name: 'Change orchestrator' })
  await expect(transfer).toBeEnabled()

  const initialRefreshGate = deferred()
  const initialObservedAt = state.runtimeInventory.observed_at_unix_ms
  state.requestLog = []
  state.inventoryResponsePlans.set('beta', [
    {
      observedAtUnixMs: initialObservedAt,
      reconcileRuntimeTimestamps: true,
      wait: initialRefreshGate.promise,
    },
  ])
  await transfer.click()
  const dialog = page.getByRole('dialog', {
    name: 'Change orchestrator',
  })
  const close = dialog.getByRole('button', {
    name: 'Close orchestrator change',
  })
  const select = dialog.getByLabel('Next orchestrator')
  await expect
    .poll(() =>
      state.requestLog.includes(
        `inventory:beta:start:${initialObservedAt}`,
      ),
    )
    .toBe(true)
  await expect(select).toBeDisabled()
  await expect(close).toBeFocused()
  expect(
    await dialog.evaluate((element) =>
      element.contains(document.activeElement),
    ),
  ).toBe(true)

  initialRefreshGate.resolve()
  await expect(select).toBeEnabled()
  await select.focus()
  await expect(select).toBeFocused()

  const pollingObservedAt = initialObservedAt + 1_000
  const pollingRefreshGate = deferred()
  state.requestLog = []
  state.inventoryResponsePlans.set('beta', [
    {
      observedAtUnixMs: pollingObservedAt,
      reconcileRuntimeTimestamps: true,
      wait: pollingRefreshGate.promise,
    },
  ])
  await expect
    .poll(() =>
      state.requestLog.includes(
        `inventory:beta:start:${pollingObservedAt}`,
      ),
      { timeout: 3_000 },
    )
    .toBe(true)
  await expect(select).toBeDisabled()
  await expect(close).toBeFocused()
  expect(
    await dialog.evaluate((element) =>
      element.contains(document.activeElement),
    ),
  ).toBe(true)

  pollingRefreshGate.resolve()
  await expect(select).toBeEnabled()
  await expect(close).toBeFocused()
})

test('keeps transfer disabled across abort, reopen, and out-of-order inventory completion', async ({
  page,
}) => {
  const state = await mockApi(page)
  moveProjectTransferFixtureToSession(state, 'beta')
  const olderObservedAt =
    state.runtimeInventory.observed_at_unix_ms + 1_000
  const newerObservedAt = olderObservedAt + 1_000
  const olderGate = deferred()
  state.inventoryResponsePlans.set(
    'beta',
    Array.from({ length: 4 }, () => ({
      observedAtUnixMs: olderObservedAt,
      reconcileRuntimeTimestamps: true,
      wait: olderGate.promise,
    })),
  )

  await page.goto('/')
  state.requestLog = []
  const marker = page.locator(
    '.orchestrator-marker[data-project-id="project-1"]',
  )
  await marker.click()
  const transfer = page
    .locator('.inspector')
    .getByRole('button', { name: 'Change orchestrator' })
  await expect
    .poll(() =>
      state.requestLog.includes(
        `inventory:beta:start:${olderObservedAt}`,
      ),
    )
    .toBe(true)
  await expect(transfer).toBeDisabled()

  await page.getByRole('button', { name: 'Close details' }).click()
  await expect(page.locator('.inspector')).toHaveCount(0)
  state.inventoryResponsePlans.set('beta', [
    {
      observedAtUnixMs: newerObservedAt,
      reconcileRuntimeTimestamps: true,
    },
  ])
  await marker.click()
  const reopenedTransfer = page
    .locator('.inspector')
    .getByRole('button', { name: 'Change orchestrator' })
  await expect(reopenedTransfer).toBeEnabled()
  expect(state.requestLog).toContain(
    `inventory:beta:complete:${newerObservedAt}`,
  )

  olderGate.resolve()
  await expect
    .poll(() =>
      state.requestLog.includes(
        `inventory:beta:complete:${olderObservedAt}`,
      ),
    )
    .toBe(true)
  expect(
    state.requestLog.indexOf(
      `inventory:beta:complete:${olderObservedAt}`,
    ),
  ).toBeGreaterThan(
    state.requestLog.indexOf(
      `inventory:beta:complete:${newerObservedAt}`,
    ),
  )
  await expect(reopenedTransfer).toBeEnabled()
  expect(state.projectOrchestratorCommands).toHaveLength(0)
})

test('delivers and ignores an older transfer response after a newer generation', async ({
  page,
}) => {
  const state = await mockApi(page)
  moveProjectTransferFixtureToSession(state, 'beta')

  await page.goto('/')
  await page
    .locator('.orchestrator-marker[data-project-id="project-1"]')
    .click()
  const transfer = page
    .locator('.inspector')
    .getByRole('button', { name: 'Change orchestrator' })
  await expect(transfer).toBeEnabled()
  await transfer.click()
  const dialog = page.getByRole('dialog', {
    name: 'Change orchestrator',
  })
  await expect(dialog.getByLabel('Next orchestrator')).toHaveValue(
    'worker-unassigned',
  )

  const olderObservedAt =
    state.runtimeInventory.observed_at_unix_ms + 1_000
  const newerObservedAt = olderObservedAt + 1_000
  const olderGate = deferred()
  state.requestLog = []
  state.inventoryResponsePlans.set('beta', [
    {
      observedAtUnixMs: olderObservedAt,
      reconcileRuntimeTimestamps: true,
      wait: olderGate.promise,
    },
  ])
  await expect
    .poll(() =>
      state.requestLog.includes(
        `inventory:beta:start:${olderObservedAt}`,
      ),
      { timeout: 3_000 },
    )
    .toBe(true)
  await expect(dialog.getByLabel('Next orchestrator')).toBeDisabled()

  state.inventoryResponsePlans.set('beta', [
    {
      observedAtUnixMs: newerObservedAt,
      reconcileRuntimeTimestamps: true,
    },
  ])
  await dialog.locator('form').evaluate((form) => {
    if (!(form instanceof HTMLFormElement)) {
      throw new Error('Transfer form is unavailable')
    }
    form.requestSubmit()
  })
  await expect
    .poll(() => state.projectOrchestratorCommands.length)
    .toBe(1)
  await expect(dialog).toHaveCount(0)
  const newerCompleteIndex = state.requestLog.findIndex(
    (entry) =>
      entry === `inventory:beta:complete:${newerObservedAt}`,
  )
  expect(newerCompleteIndex).toBeGreaterThanOrEqual(0)

  olderGate.resolve()
  await expect
    .poll(() =>
      state.requestLog.includes(
        `inventory:beta:complete:${olderObservedAt}`,
      ),
    )
    .toBe(true)
  const olderCompleteIndex = state.requestLog.findIndex(
    (entry) =>
      entry === `inventory:beta:complete:${olderObservedAt}`,
  )
  expect(olderCompleteIndex).toBeGreaterThan(newerCompleteIndex)
  await expect
    .poll(() =>
      state.requestLog.findIndex(
        (entry, index) =>
          index > olderCompleteIndex &&
          entry.startsWith('inventory:beta:start:'),
      ),
      { timeout: 3_000 },
    )
    .toBeGreaterThan(olderCompleteIndex)
  const nextBetaInventoryIndex = state.requestLog.findIndex(
    (entry, index) =>
      index > olderCompleteIndex &&
      entry.startsWith('inventory:beta:start:'),
  )
  const deliveredProjectReadIndex = state.requestLog.findIndex(
    (entry, index) =>
      index > olderCompleteIndex && entry === 'project:project-1',
  )
  const deliveredWorkerReadIndex = state.requestLog.findIndex(
    (entry, index) =>
      index > olderCompleteIndex && entry === 'workers',
  )
  expect(deliveredProjectReadIndex).toBeGreaterThan(olderCompleteIndex)
  expect(deliveredProjectReadIndex).toBeLessThan(nextBetaInventoryIndex)
  expect(deliveredWorkerReadIndex).toBeGreaterThan(olderCompleteIndex)
  expect(deliveredWorkerReadIndex).toBeLessThan(nextBetaInventoryIndex)
  expect(state.projectOrchestratorCommands).toHaveLength(1)
  await expect(
    page
      .locator('.inspector')
      .getByRole('region', { name: 'Orchestrator runtime' })
      .locator('.detail-row')
      .filter({ hasText: 'Worker ID' }),
  ).toContainText('worker-unassigned')
})

test('rejects a lower transfer timestamp until an equal observation completes', async ({
  page,
}) => {
  const state = await mockApi(page)
  moveProjectTransferFixtureToSession(state, 'beta')
  const currentObservedAt =
    state.runtimeInventory.observed_at_unix_ms + 2_000
  state.inventoryResponsePlans.set('beta', [
    {
      observedAtUnixMs: currentObservedAt,
      reconcileRuntimeTimestamps: true,
    },
  ])

  await page.goto('/')
  await page
    .locator('.orchestrator-marker[data-project-id="project-1"]')
    .click()
  const inspector = page.locator('.inspector')
  const transfer = inspector.getByRole('button', {
    name: 'Change orchestrator',
  })
  await expect(transfer).toBeEnabled()

  const lowerObservedAt = currentObservedAt - 1_000
  const lowerGate = deferred()
  const equalGate = deferred()
  state.inventoryResponsePlans.set('beta', [
    {
      observedAtUnixMs: lowerObservedAt,
      wait: lowerGate.promise,
    },
    {
      observedAtUnixMs: currentObservedAt,
      reconcileRuntimeTimestamps: true,
      wait: equalGate.promise,
    },
  ])
  await expect
    .poll(() =>
      state.requestLog.includes(
        `inventory:beta:start:${lowerObservedAt}`,
      ),
      { timeout: 3_000 },
    )
    .toBe(true)
  await expect(transfer).toBeDisabled()
  lowerGate.resolve()
  await expect(inspector.getByRole('status')).toContainText(
    'Runtime inventory is older than the current transfer snapshot',
  )
  await expect(transfer).toBeDisabled()

  await expect
    .poll(() =>
      state.requestLog.includes(
        `inventory:beta:start:${currentObservedAt}`,
      ),
      { timeout: 3_000 },
    )
    .toBe(true)
  await expect(transfer).toBeDisabled()
  equalGate.resolve()
  await expect
    .poll(() =>
      state.requestLog.includes(
        `inventory:beta:complete:${currentObservedAt}`,
      ),
    )
    .toBe(true)
  await expect(transfer).toBeEnabled()
  expect(state.projectOrchestratorCommands).toHaveLength(0)
})

test('rejects a transfer candidate with the current orchestrator worker id', async ({
  page,
}) => {
  const state = await mockApi(page)
  const candidate = state.workerCandidates.find(
    ({ worker: candidateWorker }) =>
      candidateWorker.id === 'worker-unassigned',
  )
  if (!candidate) throw new Error('Same-worker fixture is incomplete')
  candidate.worker = {
    ...candidate.worker,
    id: state.projects[0].orchestrator.id,
  }

  await page.goto('/')
  await page
    .locator('.orchestrator-marker[data-project-id="project-1"]')
    .click()
  const inspector = page.locator('.inspector')
  await expect(
    inspector.getByRole('button', { name: 'Change orchestrator' }),
  ).toBeDisabled()
  await expect(inspector.getByRole('status')).toContainText(
    'Start or free a live worker in this project workspace',
  )
  expect(state.projectOrchestratorCommands).toHaveLength(0)
})

test('disables transfer for stale candidate topology and exposes an associated action', async ({
  page,
}) => {
  const state = await mockApi(page)
  state.runtimeInventory.workers = state.runtimeInventory.workers.map(
    (observed) =>
      observed.terminal_id === 'terminal-2'
        ? { ...observed, pane_id: 'workspace-1:pane-stale' }
        : observed,
  )

  await page.setViewportSize({ width: 320, height: 844 })
  await page.goto('/')
  await page
    .locator('.orchestrator-marker[data-project-id="project-1"]')
    .click()

  const inspector = page.locator('.inspector')
  const transfer = inspector.getByRole('button', {
    name: 'Change orchestrator',
  })
  const status = inspector.getByRole('status')
  await expect(transfer).toBeDisabled()
  await expect(status).toContainText(
    'Start or free a live worker in this project workspace, then refresh',
  )
  const statusId = await status.getAttribute('id')
  expect(statusId).not.toBeNull()
  await expect(transfer).toHaveAttribute(
    'aria-describedby',
    statusId ?? '',
  )
  await status.focus()
  await expect(status).toBeFocused()
  const overflow = await page.evaluate(() => ({
    document:
      document.documentElement.scrollWidth -
      document.documentElement.clientWidth,
    inspector:
      (document.querySelector<HTMLElement>('.inspector')?.scrollWidth ?? 1) -
      (document.querySelector<HTMLElement>('.inspector')?.clientWidth ?? 0),
  }))
  expect(overflow.document).toBeLessThanOrEqual(0)
  expect(overflow.inspector).toBeLessThanOrEqual(0)
})

test('disables transfer for stale current orchestrator topology', async ({
  page,
}) => {
  const state = await mockApi(page)
  state.runtimeInventory.workers = state.runtimeInventory.workers.map(
    (observed) =>
      observed.terminal_id === 'terminal-1'
        ? { ...observed, tab_id: 'workspace-1:tab-stale' }
        : observed,
  )

  await page.goto('/')
  await page
    .locator('.orchestrator-marker[data-project-id="project-1"]')
    .click()

  const inspector = page.locator('.inspector')
  await expect(
    inspector.getByRole('button', { name: 'Change orchestrator' }),
  ).toBeDisabled()
  await expect(inspector.getByRole('status')).toContainText(
    'current orchestrator runtime topology is stale or unavailable',
  )
})

test('disables transfer for ambiguous provider identity', async ({
  page,
}) => {
  const state = await mockApi(page)
  const candidateObservation = state.runtimeInventory.workers.find(
    (observed) => observed.terminal_id === 'terminal-2',
  )
  if (!candidateObservation?.provider_session) {
    throw new Error('Provider ambiguity fixture is incomplete')
  }
  state.runtimeInventory.workers.push({
    ...worker(42, 'workspace-1'),
    provider_session: { ...candidateObservation.provider_session },
  })

  await page.goto('/')
  await page
    .locator('.orchestrator-marker[data-project-id="project-1"]')
    .click()

  const inspector = page.locator('.inspector')
  await expect(
    inspector.getByRole('button', { name: 'Change orchestrator' }),
  ).toBeDisabled()
  await expect(inspector.getByRole('status')).toContainText(
    'Start or free a live worker in this project workspace',
  )
})

test('recovers a stale project orchestrator transfer on mobile', async ({
  page,
}, testInfo) => {
  const state = await mockApi(page, {
    projectOrchestratorStaleOnce: true,
  })
  state.projects[0].name = 'Yard'
  const eligibleIndex = state.workerCandidates.findIndex(
    (candidate) => candidate.worker.id === 'worker-unassigned',
  )
  state.workerCandidates[eligibleIndex] = {
    ...state.workerCandidates[eligibleIndex],
    profile_name: 'Ready worker',
  }
  await page.setViewportSize({ width: 320, height: 844 })
  await page.goto('/')

  await page
    .locator('.orchestrator-marker[data-project-id="project-1"]')
    .click()
  const inspector = page.locator('.inspector')
  await inspector
    .getByRole('button', { name: 'Change orchestrator' })
    .click()
  const dialog = page.getByRole('dialog', { name: 'Change orchestrator' })
  const dialogBounds = await dialog.boundingBox()
  expect(dialogBounds?.x ?? -1).toBeGreaterThanOrEqual(0)
  expect(
    dialogBounds ? dialogBounds.x + dialogBounds.width : Infinity,
  ).toBeLessThanOrEqual(320)

  await dialog
    .getByRole('button', { name: 'Change orchestrator' })
    .click()
  await expect(
    dialog.getByText(
      'Project ownership changed. Review the refreshed workers and retry.',
    ),
  ).toBeVisible()
  await expect(dialog.getByLabel('Next orchestrator')).toHaveValue(
    'worker-unassigned',
  )
  await expect.poll(() => state.projectOrchestratorCommands.length).toBe(1)
  const mountedOverflow = await dialog.evaluate((element) => ({
    dialog: element.scrollWidth - element.clientWidth,
    document:
      document.documentElement.scrollWidth -
      document.documentElement.clientWidth,
  }))
  expect(mountedOverflow.dialog).toBeLessThanOrEqual(0)
  expect(mountedOverflow.document).toBeLessThanOrEqual(0)
  await page.screenshot({
    path: testInfo.outputPath('project-orchestrator-transfer-mobile.png'),
    fullPage: true,
  })

  await dialog
    .getByRole('button', { name: 'Change orchestrator' })
    .click()
  await expect(dialog).toHaveCount(0)
  await expect.poll(() => state.projectOrchestratorCommands.length).toBe(2)
  expect(state.projectOrchestratorCommands[0].command_id).not.toBe(
    state.projectOrchestratorCommands[1].command_id,
  )
  expect(state.projectOrchestratorCommands[1]).toMatchObject({
    expected_project_version: '2',
    expected_worker_version: '2',
    expected_orchestrator_worker_version: '2',
  })
  await expect(
    inspector
      .getByRole('region', { name: 'Orchestrator runtime' })
      .locator('.detail-row')
      .filter({ hasText: 'Worker ID' }),
  ).toContainText('worker-unassigned')
  const overflow = await page.evaluate(() => ({
    document:
      document.documentElement.scrollWidth -
      document.documentElement.clientWidth,
    inspector:
      document.querySelector<HTMLElement>('.inspector')?.scrollWidth ??
      0,
    inspectorClient:
      document.querySelector<HTMLElement>('.inspector')?.clientWidth ??
      0,
  }))
  expect(overflow.document).toBeLessThanOrEqual(0)
  expect(overflow.inspector).toBeLessThanOrEqual(overflow.inspectorClient)
})

test('opens chat and terminal from an assigned worker in the worker rail', async ({
  page,
}) => {
  const state = await mockApi(page)
  const seeded = seedAssignedCandidateAssignment(state)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page.getByRole('tab', { name: 'Workers' }).click()
  await page
    .locator('.worker-row[data-worker-id="worker-assigned"]')
    .click()
  await expect(
    page.getByRole('button', { name: 'Open chat', exact: true }),
  ).toBeVisible()
  expect(state.terminalSockets).toHaveLength(0)

  await page
    .getByRole('button', { name: 'Open chat', exact: true })
    .click()
  await expect(
    page.getByRole('dialog', { name: 'Implementer' }),
  ).toBeVisible()
  await page.getByRole('button', { name: 'Close chat' }).click()
  await page
    .getByRole('button', { name: 'Open terminal', exact: true })
    .click()
  await expect.poll(() => state.terminalSockets.length).toBe(1)
  expect(new URL(state.terminalConnectionUrls[0]).pathname).toBe(
    `/api/v1/projects/project-1/assignments/${seeded.id}/terminal`,
  )
})

test('reconciles an external assignment into terminal controls without reloading projects', async ({
  page,
}) => {
  const state = await mockApi(page)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page.getByRole('tab', { name: 'Workers' }).click()
  await page
    .locator('.worker-row[data-worker-id="worker-assigned"]')
    .click()
  await expect(
    page.getByRole('button', { name: 'Open terminal', exact: true }),
  ).toHaveCount(0)

  const inventoryRequests = state.inventoryRequests
  const projectRequests = state.projectRequests
  const assignmentRequests = state.assignmentRequests
  const seeded = seedAssignedCandidateAssignment(state)
  await expect
    .poll(() => state.inventoryRequests, { timeout: 3_000 })
    .toBeGreaterThan(inventoryRequests)
  await page.waitForTimeout(1_200)
  expect(state.assignmentRequests).toBe(assignmentRequests)

  const openTerminal = page.getByRole('button', {
    name: 'Open terminal',
    exact: true,
  })
  await expect(openTerminal).toBeVisible({ timeout: 7_000 })
  expect(state.assignmentRequests).toBeGreaterThan(assignmentRequests)
  expect(state.projectRequests).toBe(projectRequests)
  await openTerminal.click()
  await expect.poll(() => state.terminalSockets.length).toBe(1)
  expect(new URL(state.terminalConnectionUrls[0]).pathname).toBe(
    `/api/v1/projects/project-1/assignments/${seeded.id}/terminal`,
  )
})

test('opens an existing worker terminal in Ghostty', async ({ page }) => {
  const state = await mockApi(page)
  const seeded = seedAssignedCandidateAssignment(state)
  const runtime = seeded.worker.runtime
  if (!runtime) throw new Error('Seeded worker runtime is missing')
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page.getByRole('tab', { name: 'Workers' }).click()
  await page
    .locator('.worker-row[data-worker-id="worker-assigned"]')
    .click()
  const controls = page.getByRole('region', { name: 'Agent controls' })
  await expect(
    controls.getByText(runtime.terminal_id, { exact: true }),
  ).toBeVisible()
  await controls.getByRole('button', { name: 'Open in Ghostty' }).click()

  await expect.poll(() => state.ghosttyRequests).toEqual([
    {
      session: runtime.session,
      terminalId: runtime.terminal_id,
    },
  ])
  await expect(
    page.getByText('Opened the live terminal in Ghostty.'),
  ).toBeVisible()
  expect(state.terminalSockets).toHaveLength(0)
})

test('submits a direct prompt payload and reports only acknowledgement', async ({
  page,
}) => {
  const state = await mockApi(page, { promptDelayMs: 200 })
  const seeded = seedActiveAssignment(state)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page.locator('.assigned-worker-marker').click()
  await page
    .getByRole('button', { name: 'Open chat', exact: true })
    .click()
  const prompt = page.getByLabel('Message', { exact: true })
  const submit = page.getByRole('button', {
    name: 'Send order',
    exact: true,
  })
  await expect(submit).toBeDisabled()
  await page
    .getByLabel('Order template', { exact: true })
    .selectOption({ label: 'Pick up work' })
  await expect(prompt).toHaveValue(
    /Objective: Pick up the highest-priority unfinished work/,
  )
  await prompt.fill('  Re-run the focused test and report the result.  ')
  await expect(submit).toBeEnabled()
  await submit.click()
  await expect(submit).toBeDisabled()

  const acknowledgement = page.locator(
    '.chat-delivery-feedback[data-kind="success"]',
  )
  await expect(acknowledgement).toHaveText(
    'Order delivered to the agent.',
  )
  await expect(prompt).toHaveValue('')
  expect(state.promptCommands).toHaveLength(1)
  expect(state.promptCommands[0]).toMatchObject({
    actor: 'local-user',
    attempt_id: seeded.attempt.id,
    expected_assignment_version: seeded.version,
    expected_attempt_version: seeded.attempt.version,
  })
  expect(state.promptCommands[0].text).toContain(
    'Re-run the focused test and report the result.',
  )
  expect(state.promptCommands[0].text).toContain(
    'Need from you: <the smallest decision, credential, or input required>',
  )
  expect(state.promptCommands[0].command_id).toBeTruthy()
})

test('keeps delayed prompt results scoped to the original chat target', async ({
  page,
}) => {
  const state = await mockApi(page, { promptDelayMs: 500 })
  seedActiveAssignment(state)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page.locator('.assigned-worker-marker').click()
  await page
    .getByRole('button', { name: 'Open chat', exact: true })
    .click()
  const order = 'Finish the worker check without updating another chat.'
  await page.getByLabel('Message', { exact: true }).fill(order)
  await page
    .getByRole('button', { name: 'Send order', exact: true })
    .click()
  await expect.poll(() => state.promptCommands).toHaveLength(1)

  await page
    .locator('.agent-window-row')
    .filter({ hasText: 'API migration orchestrator' })
    .click()
  await expect(page.locator('.agent-workspace-toolbar')).toContainText(
    'API migration orchestrator',
  )
  await page.waitForTimeout(650)

  await expect(page.locator('.chat-delivery-feedback')).toHaveCount(0)
  await expect(page.getByText(order, { exact: true })).toHaveCount(0)
  expect(state.promptCommands).toHaveLength(1)
})

test('selects multiple agents and broadcasts one sourced group order', async ({
  page,
}, testInfo) => {
  const state = await mockApi(page, {
    terminalOutputText: (readCount) =>
      `Recent agent activity ${readCount}`,
  })
  seedActiveAssignment(state)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await page
    .locator('.orchestrator-marker[data-project-id="project-1"]')
    .click()
  await page
    .locator('.assigned-worker-marker')
    .click({ modifiers: ['Shift'] })

  await expect(
    page.getByRole('heading', { name: 'Group chat' }),
  ).toBeVisible()
  await expect(page.locator('.group-chat-roster > span')).toHaveCount(2)
  await page
    .getByRole('button', { name: 'Open group chat', exact: true })
    .click()
  const groupChat = page.getByRole('dialog', { name: 'Group chat' })
  const groupChatBounds = await groupChat.boundingBox()
  const closeChatBounds = await groupChat
    .getByRole('button', { name: 'Close chat' })
    .boundingBox()
  expect(
    await groupChat.evaluate(
      (element) => element.parentElement?.parentElement === document.body,
    ),
  ).toBe(true)
  expect(groupChatBounds?.y).toBeGreaterThanOrEqual(0)
  expect(closeChatBounds?.y).toBeGreaterThanOrEqual(groupChatBounds?.y ?? 0)
  await expect(
    groupChat.getByRole('button', { name: 'Close chat' }),
  ).toBeInViewport()
  expect(
    await groupChat
      .getByRole('button', { name: 'Close chat' })
      .evaluate((element) => {
        const bounds = element.getBoundingClientRect()
        const topElement = document.elementFromPoint(
          bounds.left + bounds.width / 2,
          bounds.top + bounds.height / 2,
        )
        return topElement === element || element.contains(topElement)
      }),
  ).toBe(true)
  const thread = page.getByLabel('Combined recent agent messages')
  await expect(thread.getByText('Implementer')).toBeVisible()
  await expect(
    thread.getByText('API migration orchestrator'),
  ).toBeVisible()
  await expect(
    thread.getByText(/^Recent agent activity \d+$/),
  ).toHaveCount(2)
  expect(state.terminalOutputRequests).toHaveLength(1)

  const prompt = page.getByLabel('Message', { exact: true })
  await page
    .getByLabel('Order template', { exact: true })
    .selectOption({ label: 'Status check' })
  await expect(prompt).toHaveValue(/Give a concise operational status/)
  await prompt.fill('Report current status and continue the next task.')
  await page
    .getByRole('button', { name: 'Send to 2', exact: true })
    .click()

  await expect(
    page.locator('.group-delivery-results [data-state="delivered"]'),
  ).toHaveCount(2)
  expect(state.promptCommands).toHaveLength(1)
  expect(state.orchestratorPromptCommands).toHaveLength(1)
  expect(state.promptCommands[0].text).toContain(
    'Report current status and continue the next task.',
  )
  expect(state.promptCommands[0].text).toContain('MANUAL INTERVENTION')
  expect(state.orchestratorPromptCommands[0].text).toContain(
    'Report current status and continue the next task.',
  )
  expect(state.promptCommands[0].command_id).not.toBe(
    state.orchestratorPromptCommands[0].command_id,
  )
  await expect(
    thread.locator('.chat-message[data-kind="user"]'),
  ).toContainText('2 recipients')
  await page.screenshot({
    path: testInfo.outputPath('group-chat-desktop.png'),
    fullPage: true,
  })
})

test('reuses a direct prompt command after a lost response without automatic retry', async ({
  page,
}) => {
  const state = await mockApi(page, { promptLosesResponseOnce: true })
  seedActiveAssignment(state)
  await page.setViewportSize({ width: 1200, height: 760 })
  await page.goto('/')

  await page.locator('.assigned-worker-marker').click()
  await page
    .getByRole('button', { name: 'Open chat', exact: true })
    .click()
  const prompt = page.getByLabel('Message', { exact: true })
  const submit = page.getByRole('button', {
    name: 'Send order',
    exact: true,
  })
  const retainedText = '  Check the failed deployment before proceeding.  '
  await prompt.fill(retainedText)
  await submit.click()

  await expect(
    page.getByText('Herdr acknowledgement unavailable.'),
  ).toBeVisible()
  await expect(prompt).toHaveValue(retainedText)
  await page.waitForTimeout(300)
  expect(state.promptCommands).toHaveLength(1)
  expect(state.promptDeliveries).toHaveLength(1)
  const retainedCommandId = state.promptRequestCommandIds[0]

  await submit.click()
  await expect(
    page.getByText('Order delivered to the agent.'),
  ).toBeVisible()
  await expect(prompt).toHaveValue('')
  expect(state.promptRequestCommandIds).toHaveLength(2)
  expect(state.promptRequestCommandIds[1]).toBe(retainedCommandId)
  expect(state.promptDeliveries).toHaveLength(1)
  expect(state.promptReplayCommandIds).toEqual([retainedCommandId])
})

test('editing a direct prompt after a lost response creates a fresh command', async ({
  page,
}) => {
  const state = await mockApi(page, { promptLosesResponseOnce: true })
  seedActiveAssignment(state)
  await page.setViewportSize({ width: 1200, height: 760 })
  await page.goto('/')

  await page.locator('.assigned-worker-marker').click()
  await page
    .getByRole('button', { name: 'Open chat', exact: true })
    .click()
  const prompt = page.getByLabel('Message', { exact: true })
  const submit = page.getByRole('button', {
    name: 'Send order',
    exact: true,
  })
  await prompt.fill('Inspect the deployment logs.')
  await submit.click()
  await expect(
    page.getByText('Herdr acknowledgement unavailable.'),
  ).toBeVisible()
  const originalCommandId = state.promptRequestCommandIds[0]

  await prompt.fill('Inspect the deployment logs and summarize the failure.')
  await expect(
    page.getByText('Herdr acknowledgement unavailable.'),
  ).toBeHidden()
  await submit.click()
  await expect(
    page.getByText('Order delivered to the agent.'),
  ).toBeVisible()
  expect(state.promptRequestCommandIds).toHaveLength(2)
  expect(state.promptRequestCommandIds[1]).not.toBe(originalCommandId)
  expect(state.promptDeliveries).toHaveLength(2)
  expect(state.promptReplayCommandIds).toHaveLength(0)
})

test('offers a fresh command after a durable ambiguous prompt outcome', async ({
  page,
}) => {
  const state = await mockApi(page, {
    promptDurableFailureOnce: 'command_outcome_ambiguous',
  })
  seedActiveAssignment(state)
  await page.setViewportSize({ width: 1200, height: 760 })
  await page.goto('/')

  await page.locator('.assigned-worker-marker').click()
  await page
    .getByRole('button', { name: 'Open chat', exact: true })
    .click()
  const prompt = page.getByLabel('Message', { exact: true })
  const submit = page.getByRole('button', {
    name: 'Send order',
    exact: true,
  })
  const retainedText = 'Check whether the runtime accepted this prompt.'
  await prompt.fill(retainedText)
  await submit.click()

  await expect(
    page.getByText('Herdr acknowledgement unavailable.'),
  ).toBeVisible()
  await expect(
    page
      .getByRole('alert')
      .getByText(
        'Synthetic durable prompt failure: command_outcome_ambiguous',
      ),
  ).toBeVisible()
  await expect(prompt).toHaveValue(retainedText)
  await expect(submit).toBeDisabled()
  const sendAsNew = page.getByRole('button', {
    name: 'Send as new command',
    exact: true,
  })
  await expect(sendAsNew).toBeVisible()
  await page.waitForTimeout(300)
  expect(state.promptCommands).toHaveLength(1)
  const ambiguousCommandId = state.promptRequestCommandIds[0]

  await sendAsNew.click()
  await expect(
    page.getByText('Order delivered to the agent.'),
  ).toBeVisible()
  await expect(prompt).toHaveValue('')
  expect(state.promptRequestCommandIds).toHaveLength(2)
  expect(state.promptRequestCommandIds[1]).not.toBe(ambiguousCommandId)
})

test('keeps assignment intervention controls within the mobile inspector', async ({
  page,
}, testInfo) => {
  const state = await mockApi(page, {
    promptDurableFailureOnce: 'runtime_intervention_ambiguous',
  })
  seedActiveAssignment(state)
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/')

  await page.locator('.assigned-worker-marker').click()
  await page
    .getByRole('button', { name: 'Open terminal', exact: true })
    .click()
  const terminal = page.getByLabel('Interactive terminal')
  await expect(terminal).toBeVisible()
  await expect.poll(() => state.terminalSockets.length).toBe(1)
  state.terminalSockets[0].send(
    JSON.stringify({
      type: 'terminal.frame',
      bytes: Buffer.from(
        `${'unbroken-output-token'.repeat(40)}\r\nready`,
      ).toString('base64'),
      seq: 1,
      width: 80,
      height: 24,
      full: true,
    }),
  )
  await expect(page.locator('.terminal-session')).toHaveAttribute(
    'data-frame-sequence',
    '1',
  )
  expect(state.terminalOutputRequests).toHaveLength(0)
  const terminalOverflow = await page.evaluate(() => {
    const workspace = document.querySelector<HTMLElement>(
      '.agent-workspace-shell',
    )
    const terminal = document.querySelector<HTMLElement>(
      '.terminal-session',
    )
    return {
      documentHorizontal:
        document.documentElement.scrollWidth - window.innerWidth,
      terminalHorizontal: terminal
        ? terminal.scrollWidth - terminal.clientWidth
        : Number.POSITIVE_INFINITY,
      workspaceBottom:
        workspace?.getBoundingClientRect().bottom ?? Number.POSITIVE_INFINITY,
      workspaceRight:
        workspace?.getBoundingClientRect().right ?? Number.POSITIVE_INFINITY,
      windowHeight: window.innerHeight,
      windowWidth: window.innerWidth,
    }
  })
  expect(terminalOverflow.documentHorizontal).toBeLessThanOrEqual(0)
  expect(terminalOverflow.terminalHorizontal).toBeLessThanOrEqual(0)
  expect(terminalOverflow.workspaceBottom).toBeLessThanOrEqual(
    terminalOverflow.windowHeight,
  )
  expect(terminalOverflow.workspaceRight).toBeLessThanOrEqual(
    terminalOverflow.windowWidth,
  )
  await page.screenshot({
    path: testInfo.outputPath('terminal-workspace-mobile.png'),
    fullPage: true,
  })
  await page.getByRole('button', { name: 'Close terminal' }).click()

  await page
    .getByRole('button', { name: 'Open chat', exact: true })
    .click()
  const prompt = page.getByLabel('Message', { exact: true })
  await prompt.fill('unbroken-prompt-token'.repeat(30))
  await page
    .getByRole('button', { name: 'Send order', exact: true })
    .click()
  const sendAsNew = page.getByRole('button', {
    name: 'Send as new command',
    exact: true,
  })
  await expect(sendAsNew).toBeVisible()

  const overflow = await page.evaluate(() => {
    const workspace = document.querySelector<HTMLElement>('.chat-workspace')
    const prompt = document.querySelector<HTMLTextAreaElement>(
      '.chat-composer textarea',
    )
    const sendAsNew = document.querySelector<HTMLButtonElement>(
      '.chat-delivery-feedback .secondary-button',
    )
    return {
      documentHorizontal:
        document.documentElement.scrollWidth - window.innerWidth,
      documentVertical:
        document.documentElement.scrollHeight - window.innerHeight,
      workspaceHorizontal: workspace
        ? workspace.scrollWidth - workspace.clientWidth
        : Number.POSITIVE_INFINITY,
      promptRight: prompt?.getBoundingClientRect().right ?? Infinity,
      sendAsNewRight:
        sendAsNew?.getBoundingClientRect().right ?? Infinity,
      windowWidth: window.innerWidth,
    }
  })
  expect(overflow.documentHorizontal).toBeLessThanOrEqual(0)
  expect(overflow.documentVertical).toBeLessThanOrEqual(0)
  expect(overflow.workspaceHorizontal).toBeLessThanOrEqual(0)
  expect(overflow.promptRight).toBeLessThanOrEqual(overflow.windowWidth)
  expect(overflow.sendAsNewRight).toBeLessThanOrEqual(overflow.windowWidth)

  await sendAsNew.scrollIntoViewIfNeeded()
  await page.screenshot({
    path: testInfo.outputPath('assignment-intervention-mobile.png'),
    fullPage: true,
  })
})

test('records a durable manual completion receipt', async ({ page }) => {
  const state = await mockApi(page, { completionDelayMs: 250 })
  const artifactRef = `artifact://${'a'.repeat(240)}`
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await dragToProject(page.locator('.profile-row').first(), page)
  await page.getByLabel('Objective').fill('Ship the receipt workflow.')
  await page
    .getByRole('button', { name: 'Create worker', exact: true })
    .click()

  const activeAssignment = state.assignments[0]
  if (!activeAssignment.worker.runtime) {
    throw new Error('Allocated worker runtime is missing')
  }
  activeAssignment.worker.runtime.status = 'done'
  await page.reload()
  await page.locator('.assigned-worker-marker').click()

  await expect(page.getByText('Completion review')).toBeVisible()
  await expect(
    page.getByText('Agent reports done. Evidence-backed receipt required.'),
  ).toBeVisible()
  await expect(
    page.locator('.assigned-worker-marker').getByText('Review completion'),
  ).toBeVisible()

  await page
    .getByRole('button', { name: 'Open chat', exact: true })
    .click()
  const template = page.getByLabel('Order template')
  await template.selectOption('completion-handoff')
  await expect(page.getByLabel('Message')).toHaveValue(
    /Prepare an evidence-backed completion handoff/,
  )
  await page.getByRole('button', { name: 'Close chat' }).click()

  await page
    .getByRole('button', { name: 'Record completion', exact: true })
    .click()
  let dialog = page.getByRole('dialog', { name: 'Record completion' })
  await expect(dialog).toBeVisible()
  await dialog.getByRole('button', { name: 'Close completion' }).click()
  await expect(dialog).toHaveCount(0)

  await page
    .getByRole('button', { name: 'Record completion', exact: true })
    .click()
  dialog = page.getByRole('dialog', { name: 'Record completion' })
  await expect(dialog.getByLabel('Outcome')).toBeDisabled()
  await expect(dialog.getByLabel('Outcome')).toHaveValue('completed')
  await dialog
    .getByLabel('Summary')
    .fill('Implementation and verification are complete.')
  const submit = dialog.getByRole('button', {
    name: 'Record completion',
    exact: true,
  })
  await expect(submit).toBeDisabled()

  await dialog
    .getByLabel('Artifact references')
    .fill(`${artifactRef}\n\n  build://release-manifest  `)
  await dialog
    .getByLabel('Evidence references')
    .fill('https://ci.example/runs/42')
  await dialog
    .getByLabel('Unresolved blockers')
    .fill('Monitor the staged rollout\n\n  Confirm downstream adoption  ')
  await expect(submit).toBeEnabled()
  await submit.click()
  await expect(
    dialog.getByRole('button', { name: 'Close completion' }),
  ).toBeDisabled()
  await expect(submit).toBeDisabled()

  await expect(
    page.locator('.assigned-worker-marker[data-status="done"]'),
  ).toHaveCount(0)
  const receipt = page.getByRole('region', {
    name: 'Completion receipt',
  })
  await expect(receipt).toBeVisible()
  await expect(
    receipt.getByText(
      'Implementation and verification are complete.',
      { exact: true },
    ),
  ).toBeVisible()
  await expect(page.getByText(artifactRef, { exact: true })).toBeVisible()
  await expect(page.getByText('https://ci.example/runs/42')).toBeVisible()
  await expect(page.getByText('Monitor the staged rollout')).toBeVisible()
  await expect(page.getByText('local-user', { exact: true })).toBeVisible()
  await expect(
    page.getByRole('button', { name: 'Record completion', exact: true }),
  ).toHaveCount(0)

  expect(state.completionCommands).toHaveLength(1)
  expect(state.completionCommands[0]).toMatchObject({
    actor: 'local-user',
    attempt_id: 'assignment-1-attempt',
    expected_assignment_version: '2',
    expected_attempt_version: '2',
    outcome: 'completed',
    summary: 'Implementation and verification are complete.',
    artifact_refs: [artifactRef, 'build://release-manifest'],
    evidence_refs: ['https://ci.example/runs/42'],
    unresolved_blockers: [
      'Monitor the staged rollout',
      'Confirm downstream adoption',
    ],
  })
  expect(state.assignments[0].lifecycle).toBe('completed')
  expect(state.assignments[0].attempt.lifecycle).toBe('completed')

  const overflow = await page.evaluate(() => {
    const inspector = document.querySelector<HTMLElement>('.inspector')
    return {
      documentHorizontal:
        document.documentElement.scrollWidth - window.innerWidth,
      documentVertical:
        document.documentElement.scrollHeight - window.innerHeight,
      inspectorHorizontal: inspector
        ? inspector.scrollWidth - inspector.clientWidth
        : Number.POSITIVE_INFINITY,
    }
  })
  expect(overflow.documentHorizontal).toBeLessThanOrEqual(0)
  expect(overflow.documentVertical).toBeLessThanOrEqual(0)
  expect(overflow.inspectorHorizontal).toBeLessThanOrEqual(0)

  await page.reload()
  await expect(
    page.locator('.assigned-worker-marker'),
  ).toHaveCount(0)
  await page.getByRole('tab', { name: 'Workers' }).click()
  const resumableWorker = page.locator(
    '.worker-row[data-worker-id="assignment-1-worker"][data-availability="resumable"]',
  )
  await expect(resumableWorker).toBeVisible()
  await expect(resumableWorker).toHaveAttribute('draggable', 'true')
  await resumableWorker.press('Enter')
  await expect(page.getByText('Worker candidate')).toBeVisible()
  await expect(
    page.getByRole('button', { name: 'Resume worker', exact: true }),
  ).toBeVisible()
  await expect(page.getByText('Awaiting disposition')).toBeVisible()
  const endingCandidate = state.workerCandidates.find(
    ({ worker }) => worker.id === 'assignment-1-worker',
  )
  if (!endingCandidate) throw new Error('Completed worker candidate is missing')
  await page
    .locator('.inspector')
    .getByRole('button', { name: 'End session', exact: true })
    .click()
  const endSessionDialog = page.getByRole('dialog', {
    name: 'End session',
  })
  await expect(endSessionDialog).toContainText(
    'completed work for inspection',
  )
  await endSessionDialog
    .getByRole('button', { name: 'End session', exact: true })
    .click()
  await expect(endSessionDialog).toBeHidden()
  expect(state.endSessionCommands).toHaveLength(1)
  expect(state.endSessionCommands[0]).toMatchObject({
    actor: 'local-user',
    expected_worker_version: endingCandidate.worker.version,
    expected_runtime_version: endingCandidate.worker.runtime?.version,
  })
  await expect(
    page.locator(
      '.worker-row[data-worker-id="assignment-1-worker"][data-availability="ended"]',
    ),
  ).toBeVisible()
  expect(state.assignments[0].completion_receipt?.summary).toBe(
    'Implementation and verification are complete.',
  )

  await page.setViewportSize({ width: 390, height: 844 })
  await expect(page.locator('.inspector.has-selection')).toBeVisible()
  const mobileOverflow = await page.evaluate(() => {
    const inspector = document.querySelector<HTMLElement>('.inspector')
    return {
      documentHorizontal:
        document.documentElement.scrollWidth - window.innerWidth,
      documentVertical:
        document.documentElement.scrollHeight - window.innerHeight,
      inspectorHorizontal: inspector
        ? inspector.scrollWidth - inspector.clientWidth
        : Number.POSITIVE_INFINITY,
    }
  })
  expect(mobileOverflow.documentHorizontal).toBeLessThanOrEqual(0)
  expect(mobileOverflow.documentVertical).toBeLessThanOrEqual(0)
  expect(mobileOverflow.inspectorHorizontal).toBeLessThanOrEqual(0)
})

test('uploads and safely inspects a typed HTML artifact', async ({
  page,
}, testInfo) => {
  const state = await mockApi(page)
  const html = [
    '<!doctype html><html><head>',
    '<style>body{font-family:sans-serif}h1{color:#19766b}</style>',
    '</head><body><h1>Release report</h1>',
    '<img src="https://example.invalid/tracker.png">',
    '<script>parent.pwned = true</script></body></html>',
  ].join('')
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  await dragToProject(page.locator('.profile-row').first(), page)
  await page.getByLabel('Objective').fill('Publish a release report.')
  await page
    .getByRole('button', { name: 'Create worker', exact: true })
    .click()
  await page
    .getByRole('button', { name: 'Record completion', exact: true })
    .click()

  const completion = page.getByRole('dialog', { name: 'Record completion' })
  await completion
    .getByLabel('Summary')
    .fill('Published the release report.')
  await completion.getByLabel('Typed artifacts').setInputFiles({
    name: 'release-report.html',
    mimeType: 'text/html',
    buffer: Buffer.from(html),
  })
  await expect(completion.getByText('release-report.html')).toBeVisible()
  await completion
    .getByRole('button', { name: 'Record completion', exact: true })
    .click()

  await expect(page.getByText('Completion receipt', { exact: true })).toBeVisible()
  expect(state.artifacts.size).toBe(1)
  expect(state.completionCommands[0].artifact_ids).toHaveLength(1)
  const artifactButton = page.getByRole('button', {
    name: /release-report\.html/,
  })
  await expect(artifactButton).toBeVisible()
  await artifactButton.click()

  const inspector = page.getByRole('dialog', {
    name: 'release-report.html',
  })
  await expect(inspector).toBeVisible()
  const frame = inspector.locator('iframe')
  await expect(frame).toHaveAttribute('sandbox', '')
  await expect(frame).toHaveAttribute('referrerpolicy', 'no-referrer')
  await expect(
    frame.contentFrame().getByRole('heading', { name: 'Release report' }),
  ).toBeVisible()
  await expect(frame.contentFrame().locator('script')).toHaveCount(0)
  await expect(frame.contentFrame().locator('img')).toHaveCount(0)
  await expect(
    frame
      .contentFrame()
      .locator('meta[http-equiv="Content-Security-Policy"]'),
  ).toHaveAttribute('content', /default-src 'none'/)
  expect(await page.evaluate(() => Reflect.get(window, 'pwned'))).toBeUndefined()

  await inspector.getByRole('button', { name: 'Source' }).click()
  await expect(inspector.locator('.artifact-source')).toContainText(
    'parent.pwned = true',
  )
  await inspector.getByRole('button', { name: 'Close artifact' }).click()
  await expect(artifactButton).toBeFocused()

  await page.setViewportSize({ width: 390, height: 844 })
  await artifactButton.click()
  await expect(inspector).toBeVisible()
  const inspectorBounds = await inspector.boundingBox()
  const closeArtifactBounds = await inspector
    .getByRole('button', { name: 'Close artifact' })
    .boundingBox()
  expect(
    await inspector.evaluate(
      (element) => element.parentElement?.parentElement === document.body,
    ),
  ).toBe(true)
  expect(inspectorBounds?.y).toBeGreaterThanOrEqual(0)
  expect(closeArtifactBounds?.y).toBeGreaterThanOrEqual(
    inspectorBounds?.y ?? 0,
  )
  expect(
    await inspector
      .getByRole('button', { name: 'Close artifact' })
      .evaluate((element) => {
        const bounds = element.getBoundingClientRect()
        const topElement = document.elementFromPoint(
          bounds.left + bounds.width / 2,
          bounds.top + bounds.height / 2,
        )
        return topElement === element || element.contains(topElement)
      }),
  ).toBe(true)
  const overflow = await inspector.evaluate((element) => ({
    documentHorizontal:
      document.documentElement.scrollWidth - window.innerWidth,
    documentVertical:
      document.documentElement.scrollHeight - window.innerHeight,
    inspectorHorizontal:
      element.scrollWidth - element.clientWidth,
    inspectorVertical:
      element.scrollHeight - element.clientHeight,
  }))
  expect(overflow.documentHorizontal).toBeLessThanOrEqual(0)
  expect(overflow.documentVertical).toBeLessThanOrEqual(0)
  expect(overflow.inspectorHorizontal).toBeLessThanOrEqual(0)
  expect(overflow.inspectorVertical).toBeLessThanOrEqual(0)
  await page.screenshot({
    path: testInfo.outputPath('artifact-inspector-mobile.png'),
    fullPage: true,
  })
})

test('reports queued runtime cleanup after ending a session', async ({
  page,
}) => {
  const state = await mockApi(page, { endSessionCleanupPending: true })
  await page.goto('/')

  await page.getByRole('tab', { name: 'Workers' }).click()
  await page
    .locator(
      '.worker-row[data-worker-id="worker-resumable"][data-availability="resumable"]',
    )
    .click()
  await page
    .locator('.inspector')
    .getByRole('button', { name: 'End session', exact: true })
    .click()
  await page
    .getByRole('dialog', { name: 'End session' })
    .getByRole('button', { name: 'End session', exact: true })
    .click()

  await expect(
    page.getByText('Session ended. Verified runtime cleanup is queued.'),
  ).toBeVisible()
  await expect(
    page.locator(
      '.worker-row[data-worker-id="worker-resumable"][data-availability="ended"]',
    ),
  ).toBeVisible()
  expect(state.endSessionCommands).toHaveLength(1)
})

test('retries completion with the same command ID after a network failure', async ({
  page,
}) => {
  const state = await mockApi(page, { completionFailsOnce: true })
  await page.setViewportSize({ width: 1200, height: 760 })
  await page.goto('/')

  await dragToProject(page.locator('.profile-row').first(), page)
  await page.getByLabel('Objective').fill('Verify retry semantics.')
  await page
    .getByRole('button', { name: 'Create worker', exact: true })
    .click()
  await page
    .getByRole('button', { name: 'Record completion', exact: true })
    .click()

  const dialog = page.getByRole('dialog', { name: 'Record completion' })
  await dialog.getByLabel('Summary').fill('Retry-safe completion.')
  await dialog.getByLabel('Evidence references').fill('test://retry')
  const submit = dialog.getByRole('button', {
    name: 'Record completion',
    exact: true,
  })
  await submit.click()

  await expect(dialog).toBeVisible()
  await expect(submit).toBeEnabled()
  await submit.click()

  await expect(
    page.locator('.assigned-worker-marker[data-status="done"]'),
  ).toHaveCount(0)
  expect(state.completionRequestCommandIds).toHaveLength(2)
  expect(state.completionRequestCommandIds[0]).toBe(
    state.completionRequestCommandIds[1],
  )
})

test('creates and edits a reusable worker profile', async ({ page }) => {
  const state = await mockApi(page)
  await page.setViewportSize({ width: 1200, height: 760 })
  await page.goto('/')

  await page.getByRole('button', { name: 'Create worker profile' }).click()
  await page.getByLabel('Profile name').fill('Reviewer')
  await page.getByLabel('Default role').fill('reviewer')
  await page.getByRole('button', { name: 'Save profile' }).click()
  await expect(page.locator('.profile-row')).toHaveCount(2)

  await page.getByRole('button', { name: /Reviewer/ }).click()
  await page.getByRole('button', { name: 'Edit profile' }).click()
  await page.getByLabel('Profile name').fill('Senior reviewer')
  await page.getByRole('button', { name: 'Save profile' }).click()

  await expect(
    page.getByRole('heading', { name: 'Senior reviewer' }),
  ).toBeVisible()
  expect(state.profiles[1].name).toBe('Senior reviewer')
  expect(state.profiles[1].version).toBe('2')
})

test('traps modal focus and restores the opening control', async ({ page }) => {
  await mockApi(page)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  const trigger = page.getByRole('button', {
    name: 'Create worker profile',
  })
  await trigger.click()
  const dialog = page.getByRole('dialog', { name: 'New profile' })
  await expect(dialog).toBeVisible()
  await expect(page.locator('.app-shell')).toHaveAttribute('inert', '')
  await expect(page.locator('.app-shell')).toHaveAttribute(
    'aria-hidden',
    'true',
  )

  for (let index = 0; index < 20; index += 1) {
    await page.keyboard.press('Tab')
    expect(
      await dialog.evaluate((element) =>
        element.contains(document.activeElement),
      ),
    ).toBe(true)
  }

  await page.keyboard.press('Escape')
  await expect(dialog).toHaveCount(0)
  await expect(trigger).toBeFocused()
  await expect(page.locator('.app-shell')).not.toHaveAttribute('inert', '')
  await expect(page.locator('.app-shell')).not.toHaveAttribute(
    'aria-hidden',
    'true',
  )
})

test('creates a worker profile from a reusable role template', async ({
  page,
}) => {
  const state = await mockApi(page)
  await page.setViewportSize({ width: 1200, height: 760 })
  await page.goto('/')

  await page.getByRole('button', { name: 'Create worker profile' }).click()
  await page.getByLabel('Profile template').selectOption('verifier')
  await expect(page.getByLabel('Profile name')).toHaveValue('Verifier')
  await expect(page.getByLabel('Provider')).toHaveValue('codex')
  await expect(page.getByLabel('Model')).toHaveValue('')
  await expect(page.getByLabel('Default role')).toHaveValue('verifier')
  await expect(page.getByLabel('Instructions reference')).toHaveValue(
    'AGENTS.md',
  )
  await expect(page.getByLabel('Tools')).toHaveValue('')
  await expect(page.getByLabel('Skills')).toHaveValue('')
  await expect(page.getByLabel('MCP servers')).toHaveValue('')
  await expect(page.getByLabel('Sandbox')).toHaveValue('runtime_default')
  await expect(page.getByLabel('Worktree')).toHaveValue('project_workspace')
  await expect(page.getByLabel('Permissions')).toHaveValue('runtime_default')
  await page.getByRole('button', { name: 'Save profile' }).click()

  expect(state.profiles.at(-1)).toMatchObject({
    completion_contract: 'manual_receipt',
    default_role: 'verifier',
    instructions_ref: 'AGENTS.md',
    mcp_servers: [],
    model: null,
    name: 'Verifier',
    permission_policy: 'runtime_default',
    provider: 'codex',
    runtime_adapter: 'herdr',
    sandbox_policy: 'runtime_default',
    skills: [],
    tools: [],
    worktree_policy: 'project_workspace',
  })
})

test('persists project drag placement across reload', async ({ page }) => {
  const state = await mockApi(page)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')
  const before = { ...state.projects[0].placement.geometry }

  await dragFirstProject(page, 120, 70)
  await expect
    .poll(() => state.placementUpdates, { timeout: 5_000 })
    .toBe(1)

  // A drag straight across the screen crosses both ground axes, so both world
  // coordinates move. Anything that only changed x would mean the pointer delta
  // never went through the inverse projection.
  expect(state.projects[0].placement.geometry.x).toBeGreaterThan(before.x)
  expect(state.projects[0].placement.geometry.y).not.toBe(before.y)
  // The exact split between the two world axes is fixed by the projection and
  // is independent of the viewport zoom: for a screen delta (dx, dy),
  // (worldDx - worldDy) / (worldDx + worldDy) == (dx / 0.82) / (dy / 0.44).
  const moved = {
    x: state.projects[0].placement.geometry.x - before.x,
    y: state.projects[0].placement.geometry.y - before.y,
  }
  expect(moved.x + moved.y).not.toBeCloseTo(0, 3)
  // Tolerance covers the browser rounding pointer coordinates to whole pixels.
  // A flat, unprojected drag would land near 0.26 here, so this is a wide berth
  // around the right answer rather than a loose one.
  expect(
    Math.abs(
      (moved.x - moved.y) / (moved.x + moved.y) - 120 / 0.82 / (70 / 0.44),
    ),
  ).toBeLessThan(0.02)

  // And the magnitude follows the pointer: dividing the screen delta by the
  // live zoom and inverse projecting it reproduces the persisted delta.
  const zoom = await page.evaluate(() => {
    const viewport = document.querySelector<HTMLElement>(
      '.react-flow__viewport',
    )
    const match = viewport?.style.transform.match(/scale\(([^)]+)\)/)
    return match ? Number(match[1]) : 1
  })
  const flowDelta = { x: 120 / zoom, y: 70 / zoom }
  const ideal = {
    x: 0.5 * (flowDelta.x / 0.82 + flowDelta.y / 0.44),
    y: 0.5 * (flowDelta.y / 0.44 - flowDelta.x / 0.82),
  }
  // ReactFlow consumes the first pointer step to cross its own drag threshold,
  // in 2D view exactly as in 2.5D, so the persisted delta trails the ideal by
  // up to one step of the eight this helper sends.
  const step = {
    x: ideal.x / 8,
    y: ideal.y / 8,
  }
  expect(moved.x).toBeLessThan(ideal.x + 2)
  expect(moved.x).toBeGreaterThan(ideal.x - Math.abs(step.x) - 2)
  expect(moved.y).toBeLessThan(ideal.y + Math.abs(step.y) + 2)
  expect(moved.y).toBeGreaterThan(ideal.y - Math.abs(step.y) - 2)

  const persisted = state.projects[0].placement.geometry
  await page.reload()
  await expect(page.locator('.project-region')).toHaveCount(2)
  expect(state.projects[0].placement.geometry).toEqual(persisted)

  // Toggling the view mode must never rewrite a placement.
  await setMapView(page, '2D view')
  await setMapView(page, '2.5D view')
  expect(state.projects[0].placement.geometry).toEqual(persisted)
  expect(state.placementUpdates).toBe(1)
})

test('selects and drags a project through the projected territory itself', async ({
  page,
}) => {
  const state = await mockApi(page)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  const territoryGroup = page.locator('.projected-territory').first()
  const territory = territoryGroup.locator('.territory-polygon')
  const box = await territory.boundingBox()
  expect(box).not.toBeNull()
  // Left of centre, halfway down: inside the visible parallelogram, outside the
  // flat node rectangle, and clear of the upright label at the centroid.
  const grab = {
    x: box!.x + box!.width * 0.25,
    y: box!.y + box!.height * 0.5,
  }

  await page.mouse.move(grab.x, grab.y)
  await page.mouse.down()
  await page.mouse.up()

  // Selection made on the projected surface opens the project inspector and
  // marks the still-mounted flat node selected, so anything reading that state
  // stays consistent.
  await expect(
    page.getByRole('heading', { name: 'API migration' }),
  ).toBeVisible()
  await expect(territoryGroup).toHaveClass(/is-selected/)
  await expect(
    page.locator('.react-flow__node-project').first().locator('.project-region'),
  ).toHaveClass(/is-selected/)

  const before = { ...state.projects[0].placement.geometry }
  await page.mouse.move(grab.x, grab.y)
  await page.mouse.down()
  await page.mouse.move(grab.x + 90, grab.y + 40, { steps: 8 })
  await page.mouse.up()

  await expect
    .poll(() => state.placementUpdates, { timeout: 5_000 })
    .toBe(1)
  const moved = {
    x: state.projects[0].placement.geometry.x - before.x,
    y: state.projects[0].placement.geometry.y - before.y,
  }
  expect((moved.x - moved.y) / (moved.x + moved.y)).toBeCloseTo(
    90 / 0.82 / (40 / 0.44),
    2,
  )
  expect(state.projects[0].placement.geometry.width).toBe(before.width)
  expect(state.projects[0].placement.geometry.height).toBe(before.height)
})

test('resizes a project on the flat box and settles the projected shape', async ({
  page,
}) => {
  // Documented 2.5D compromise: NodeResizer computes its handles from the
  // node's own flat rectangle, so the gesture stays flat and the projected
  // territory re-renders once onResizeEnd fires. This test exists to prove the
  // interaction still functions, not that it looks projected while dragging.
  const state = await mockApi(page)
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto('/')

  const projectNode = page.locator('[data-id="project:project-1"]')
  await projectNode.dispatchEvent('click')
  const control = projectNode.locator('.react-flow__resize-control.bottom.right')
  await expect(control).toHaveCount(1)
  const handle = await control.boundingBox()
  expect(handle).not.toBeNull()

  const before = { ...state.projects[0].placement.geometry }
  const cornersBefore = await page
    .locator('.projected-territory[data-node-id="project:project-1"] .territory-polygon')
    .getAttribute('points')

  await page.mouse.move(
    handle!.x + handle!.width / 2,
    handle!.y + handle!.height / 2,
  )
  await page.mouse.down()
  await page.mouse.move(
    handle!.x + handle!.width / 2 + 80,
    handle!.y + handle!.height / 2 + 60,
    { steps: 8 },
  )
  await page.mouse.up()

  await expect.poll(() => state.projects[0].placement.geometry.width).toBeGreaterThan(
    before.width,
  )
  await expect
    .poll(async () =>
      page
        .locator(
          '.projected-territory[data-node-id="project:project-1"] .territory-polygon',
        )
        .getAttribute('points'),
    )
    .not.toBe(cornersBefore)
})

test('drops an allocation on the visibly projected territory', async ({
  page,
}) => {
  const state = await mockApi(page)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  const territory = page.locator('.territory-polygon').first()
  const box = await territory.boundingBox()
  expect(box).not.toBeNull()

  // A quarter of the way across the projected envelope, halfway down, is inside
  // the visible parallelogram but well to the left of the flat node rectangle
  // the territory used to be. Only the inverse-projected hit test can resolve
  // this drop to a project.
  const target = { x: box!.width * 0.25, y: box!.height * 0.5 }
  const flatBox = await page
    .locator('.react-flow__node-project')
    .first()
    .boundingBox()
  expect(flatBox).not.toBeNull()
  expect(box!.x + target.x).toBeLessThan(flatBox!.x)

  await page
    .locator('.profile-row')
    .first()
    .dragTo(territory, { targetPosition: target })
  await expect(
    page.getByRole('dialog', { name: 'Create worker' }),
  ).toBeVisible()
  await page.getByLabel('Objective').fill('Verify the projected drop target.')
  await page
    .getByRole('button', { name: 'Create worker', exact: true })
    .click()
  await expect.poll(() => state.assignments.length).toBe(1)
  expect(state.assignments[0].project_id).toBe('project-1')
})

test('creates a map node at the inverse-mapped right-click point', async ({
  page,
}) => {
  const state = await mockApi(page)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  const canvas = page.locator('.react-flow')
  const bounds = await canvas.boundingBox()
  expect(bounds).not.toBeNull()
  const click = { x: bounds!.x + 240, y: bounds!.y + 200 }
  await page.mouse.click(click.x, click.y, { button: 'right' })
  await page.getByRole('menuitem', { name: /Workstream/ }).click()
  await page.getByLabel('Name').fill('Projected placement')
  await page
    .getByRole('button', { name: 'Create node', exact: true })
    .click()

  await expect.poll(() => state.coordinationNodes.length).toBe(1)
  const placement = state.coordinationNodes[0].placement.geometry
  const projected = await page.evaluate(
    ({ x, y }) => {
      const viewport = document.querySelector<HTMLElement>(
        '.react-flow__viewport',
      )
      const transform = viewport?.style.transform ?? ''
      const translate = transform.match(
        /translate\(([^p]+)px,\s*([^p]+)px\)/,
      )
      const scale = transform.match(/scale\(([^)]+)\)/)
      const container = document
        .querySelector('.react-flow')!
        .getBoundingClientRect()
      const zoom = scale ? Number(scale[1]) : 1
      const flow = {
        x: (x - container.x - Number(translate?.[1] ?? 0)) / zoom,
        y: (y - container.y - Number(translate?.[2] ?? 0)) / zoom,
      }
      return {
        // The stored placement must be the unprojected world point, centred on
        // the pointer, not the raw flow point.
        x: 0.5 * (flow.x / 0.82 + flow.y / 0.44) - 58,
        y: 0.5 * (flow.y / 0.44 - flow.x / 0.82) - 58,
      }
    },
    click,
  )
  expect(placement.x).toBeCloseTo(projected.x, 0)
  expect(placement.y).toBeCloseTo(projected.y, 0)
})

test('persists agent grip placement across refresh and reload', async ({
  page,
}) => {
  const state = await mockApi(page)
  const seeded = seedActiveAssignment(state)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  const node = page.locator('.react-flow__node-assigned-worker')
  const handle = node.locator('.worker-marker__placement-handle')
  const before = await node.boundingBox()
  const handleBox = await handle.boundingBox()
  if (!before || !handleBox) throw new Error('Agent placement grip is hidden')

  const startX = handleBox.x + handleBox.width / 2
  const startY = handleBox.y + handleBox.height / 2
  await page.mouse.move(startX, startY)
  await page.mouse.down()
  await page.mouse.move(startX + 72, startY + 30, { steps: 8 })
  await page.mouse.up()

  await expect
    .poll(() =>
      page.evaluate(
        ({ key, nodeId }) => {
          const stored = window.localStorage.getItem(key)
          if (!stored) return null
          return (
            JSON.parse(stored) as Record<
              string,
              { x: number; y: number }
            >
          )[nodeId]
        },
        {
          key: 'yard:agent-positions:v1',
          nodeId: `assigned-worker:${seeded.worker.id}`,
        },
      ),
    )
    .not.toBeNull()

  const moved = await node.boundingBox()
  expect(moved?.x).toBeGreaterThan(before.x + 20)
  const inventoryRequests = state.inventoryRequests
  const runtimeHealth = await openRuntimeHealth(page)
  await runtimeHealth.getByRole('button', { name: 'Refresh state' }).click()
  await expect.poll(() => state.inventoryRequests).toBeGreaterThan(
    inventoryRequests,
  )
  await expect
    .poll(async () => (await node.boundingBox())?.x ?? 0)
    .toBeGreaterThan(before.x + 20)

  await page.reload()
  await expect(node).toBeVisible()
  await expect
    .poll(async () => (await node.boundingBox())?.x ?? 0)
    .toBeGreaterThan(before.x + 20)
})

test('persists keyboard project movement and ignores delete keys', async ({
  page,
}) => {
  const state = await mockApi(page)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  const projectNode = page.locator('.react-flow__node-project').first()
  const before = state.projects[0].placement.geometry.x
  await projectNode.focus()
  await projectNode.press('Enter')
  await expect(
    page.getByRole('heading', { name: 'API migration' }),
  ).toBeVisible()
  await projectNode.press('ArrowRight')
  await expect
    .poll(() => state.placementUpdates, { timeout: 5_000 })
    .toBe(1)
  expect(state.projects[0].placement.geometry.x).toBe(before + 5)

  await projectNode.press('Delete')
  await projectNode.press('Backspace')
  await expect(page.locator('.react-flow__node-project')).toHaveCount(2)

  const persisted = state.projects[0].placement.geometry
  await page.reload()
  expect(state.projects[0].placement.geometry).toEqual(persisted)
})

test('persists unbound workspace placement per runtime session', async ({
  page,
}) => {
  await mockApi(page)
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')

  const workspace = page.locator('.react-flow__node-workspace').first()
  const before = await workspace.evaluate(
    (element) => (element as HTMLElement).style.transform,
  )
  await dragFirstWorkspace(page, 96, 54)
  const moved = await workspace.evaluate(
    (element) => (element as HTMLElement).style.transform,
  )
  expect(moved).not.toBe(before)

  await page.reload()
  await expect(page.locator('.react-flow__node-workspace')).toHaveCount(3)
  await expect(
    page.locator('.react-flow__node-workspace').first(),
  ).toHaveCSS('transform', /matrix/)
  expect(
    await page
      .locator('.react-flow__node-workspace')
      .first()
      .evaluate((element) => (element as HTMLElement).style.transform),
  ).toBe(moved)

  await selectRuntimeSession(page, 'beta')
  await selectRuntimeSession(page, 'alpha')
  await expect(
    page.locator('.react-flow__node-workspace').first(),
  ).toHaveCSS('transform', /matrix/)
  expect(
    await page
      .locator('.react-flow__node-workspace')
      .first()
      .evaluate((element) => (element as HTMLElement).style.transform),
  ).toBe(moved)
})

test('rolls project geometry back after a placement conflict', async ({
  page,
}) => {
  const state = await mockApi(page, { conflictNextPlacement: true })
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto('/')
  const original = { ...state.projects[0].placement.geometry }

  await dragFirstProject(page, 140, 80)

  await expect(
    page.getByText(
      'Project placement changed concurrently; current version is 2',
    ),
  ).toBeVisible()
  await expect
    .poll(() => state.placementUpdates, { timeout: 5_000 })
    .toBe(1)
  expect(state.projects[0].placement.geometry).toEqual(original)
})

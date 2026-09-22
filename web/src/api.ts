import type {
  ArchiveProjectInput,
  ArchivedProject,
  Artifact,
  ArtifactContent,
  Automation,
  AutomationRun,
  AutomationRuns,
  Automations,
  Assignments,
  ChangedProjectOrchestrator,
  ChangeProjectOrchestratorInput,
  CompletedRuntimeCleanupPreview,
  ConfirmedAllocation,
  ConfirmedProjectCreation,
  ConfirmedWorkerHandoff,
  ConfiguredYardOrchestrator,
  CoordinationNodeCommandResult,
  CoordinationNodePromptAcknowledgement,
  CoordinationNodeRoute,
  CoordinationNodeRoutes,
  CoordinationNodes,
  CoordinationNodeTerminalOutput,
  CoordinationSnapshot,
  CoordinationSnapshots,
  CreatedProjectRelationship,
  ConfirmWorkerAssignmentInput,
  ConfirmWorkerHandoffInput,
  ConfigureYardOrchestratorInput,
  CreateCoordinationNodeInput,
  CreateAutomationInput,
  CreateProjectRelationshipInput,
  CreateProjectInput,
  CreateProjectFromProfileInput,
  CreateWorkerProfileInput,
  CreateWorkspaceProjectFromProfileInput,
  DeletedProject,
  DeletedProjectRelationship,
  DeletedWorker,
  DeleteProjectInput,
  DeleteProjectRelationshipInput,
  DeleteWorkerInput,
  EndedWorkerSession,
  EndWorkerSessionInput,
  ExternalTerminalLaunch,
  HerdrFleetInventory,
  OrchestratorPromptAcknowledgement,
  OrchestratorTerminalOutput,
  OrchestratorWorkflowProfile,
  Project,
  Projects,
  ProvisionYardOrchestratorInput,
  RecoverYardOrchestratorInput,
  RecoveredYardOrchestrator,
  ResetOrchestratorWorkflowProfileInput,
  ProvisionCoordinationNodeInput,
  PromptAcknowledgement,
  RecordedCompletionReceipt,
  RecordCompletionReceiptInput,
  RuntimeInventory,
  RuntimeLens,
  RuntimeSessions,
  RuntimeTopology,
  RunAutomationInput,
  SendAssignmentPromptInput,
  SendCoordinationNodePromptInput,
  SendCoordinationNodeRouteInput,
  SendOrchestratorPromptInput,
  SendYardOrchestratorPromptInput,
  SendYardOrchestratorRouteInput,
  TerminalOutput,
  TokenSpendSettings,
  UpdateCoordinationNodeInput,
  UpdateCoordinationNodePlacementInput,
  UpdateAutomationInput,
  UpdateAutomationPlacementInput,
  UpdateAutomationStateInput,
  UpdateOrchestratorWorkflowProfileInput,
  UpdateProjectPlacementInput,
  UpdateTokenSpendSettingsInput,
  UpdateWorkerProfileInput,
  UploadArtifactInput,
  RequestCoordinationSnapshotInput,
  WorkerCandidates,
  WorkerProfile,
  WorkerProfiles,
  YardOrchestrator,
  YardOrchestratorPromptAcknowledgement,
  YardOrchestratorRoute,
  YardOrchestratorRoutes,
  YardOrchestratorTerminalOutput,
  ProjectRelationships,
} from './types'

interface ApiErrorEnvelope {
  error?: {
    code?: string
    message?: string
    attempted_session_count?: unknown
    failed_session_count?: unknown
  }
}

export class YardApiError extends Error {
  code: string
  attemptedSessions: number | null
  failedSessions: number | null

  constructor(
    code: string,
    message: string,
    attemptedSessions: number | null = null,
    failedSessions: number | null = null,
  ) {
    super(message)
    this.name = 'YardApiError'
    this.code = code
    this.attemptedSessions = attemptedSessions
    this.failedSessions = failedSessions
  }
}

async function requestJson<T>(
  path: string,
  init: RequestInit = {},
): Promise<T> {
  const response = await fetch(path, {
    ...init,
    headers: {
      Accept: 'application/json',
      ...init.headers,
    },
  })

  if (!response.ok) {
    let body: ApiErrorEnvelope = {}
    try {
      body = (await response.json()) as ApiErrorEnvelope
    } catch {
      // Preserve the HTTP fallback below when an intermediary returns text.
    }
    const attemptedSessions = body.error?.attempted_session_count
    const failedSessions = body.error?.failed_session_count
    throw new YardApiError(
      body.error?.code ?? `http_${response.status}`,
      body.error?.message ?? `Yard returned HTTP ${response.status}`,
      typeof attemptedSessions === 'number' &&
        Number.isSafeInteger(attemptedSessions) &&
        attemptedSessions >= 0
        ? attemptedSessions
        : null,
      typeof failedSessions === 'number' &&
        Number.isSafeInteger(failedSessions) &&
        failedSessions >= 0
        ? failedSessions
        : null,
    )
  }

  return (await response.json()) as T
}

export function fetchSessions(signal?: AbortSignal): Promise<RuntimeSessions> {
  return requestJson('/api/v1/runtimes/herdr/sessions', { signal })
}

export function fetchHerdrFleetInventory(
  signal?: AbortSignal,
): Promise<HerdrFleetInventory> {
  return requestJson('/api/v1/runtimes/herdr/inventory', { signal })
}

export function fetchInventory(
  session: string,
  signal?: AbortSignal,
): Promise<RuntimeInventory> {
  return requestJson(
    `/api/v1/runtimes/herdr/sessions/${encodeURIComponent(session)}/inventory`,
    { signal },
  )
}

export function fetchRuntimeLens(
  session: string,
  signal?: AbortSignal,
): Promise<RuntimeLens> {
  return requestJson(
    `/api/v1/runtimes/herdr/sessions/${encodeURIComponent(session)}/lens`,
    { signal },
  )
}

export function fetchRuntimeTopology(
  session: string,
  signal?: AbortSignal,
): Promise<RuntimeTopology> {
  return requestJson(
    `/api/v1/runtimes/herdr/sessions/${encodeURIComponent(session)}/topology`,
    { signal },
  )
}

export function fetchProjects(signal?: AbortSignal): Promise<Projects> {
  return requestJson('/api/v1/projects', { signal })
}

export function fetchProject(
  projectId: string,
  signal?: AbortSignal,
): Promise<Project> {
  return requestJson(
    `/api/v1/projects/${encodeURIComponent(projectId)}`,
    { signal },
  )
}

export function archiveProject(
  projectId: string,
  command: ArchiveProjectInput,
  signal?: AbortSignal,
): Promise<ArchivedProject> {
  return requestJson(
    `/api/v1/projects/${encodeURIComponent(projectId)}/archive`,
    {
      body: JSON.stringify(command),
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal,
    },
  )
}

export function deleteProject(
  projectId: string,
  command: DeleteProjectInput,
  signal?: AbortSignal,
): Promise<DeletedProject> {
  return requestJson(
    `/api/v1/projects/${encodeURIComponent(projectId)}/delete`,
    {
      body: JSON.stringify(command),
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal,
    },
  )
}

export function changeProjectOrchestrator(
  projectId: string,
  command: ChangeProjectOrchestratorInput,
  signal?: AbortSignal,
): Promise<ChangedProjectOrchestrator> {
  return requestJson(
    `/api/v1/projects/${encodeURIComponent(projectId)}/orchestrator`,
    {
      body: JSON.stringify(command),
      headers: { 'Content-Type': 'application/json' },
      method: 'PUT',
      signal,
    },
  )
}

export function fetchProjectRelationships(
  signal?: AbortSignal,
): Promise<ProjectRelationships> {
  return requestJson('/api/v1/project-relationships', { signal })
}

export function fetchCoordinationNodes(
  signal?: AbortSignal,
): Promise<CoordinationNodes> {
  return requestJson('/api/v1/coordination-nodes', { signal })
}

type AutomationEnvelope = Automation | { automation: Automation }
type AutomationRunEnvelope = AutomationRun | { run: AutomationRun }

function unwrapAutomation(result: AutomationEnvelope): Automation {
  return 'automation' in result ? result.automation : result
}

function unwrapAutomationRun(result: AutomationRunEnvelope): AutomationRun {
  return 'run' in result ? result.run : result
}

export function fetchAutomations(signal?: AbortSignal): Promise<Automations> {
  return requestJson('/api/v1/automations', { signal })
}

export function fetchTokenSpendSettings(
  signal?: AbortSignal,
): Promise<TokenSpendSettings> {
  return requestJson('/api/v1/token-spend-settings', { signal })
}

export function fetchOrchestratorWorkflowProfile(
  signal?: AbortSignal,
): Promise<OrchestratorWorkflowProfile> {
  return requestJson('/api/v1/orchestrator-workflow-profile', { signal })
}

export function updateOrchestratorWorkflowProfile(
  command: UpdateOrchestratorWorkflowProfileInput,
  signal?: AbortSignal,
): Promise<OrchestratorWorkflowProfile> {
  return requestJson('/api/v1/orchestrator-workflow-profile', {
    body: JSON.stringify(command),
    headers: { 'Content-Type': 'application/json' },
    method: 'PUT',
    signal,
  })
}

export function resetOrchestratorWorkflowProfile(
  command: ResetOrchestratorWorkflowProfileInput,
  signal?: AbortSignal,
): Promise<OrchestratorWorkflowProfile> {
  return requestJson('/api/v1/orchestrator-workflow-profile/reset', {
    body: JSON.stringify(command),
    headers: { 'Content-Type': 'application/json' },
    method: 'POST',
    signal,
  })
}

export function updateTokenSpendSettings(
  command: UpdateTokenSpendSettingsInput,
  signal?: AbortSignal,
): Promise<TokenSpendSettings> {
  return requestJson('/api/v1/token-spend-settings', {
    body: JSON.stringify(command),
    headers: { 'Content-Type': 'application/json' },
    method: 'PUT',
    signal,
  })
}

export function createAutomation(
  command: CreateAutomationInput,
  signal?: AbortSignal,
): Promise<Automation> {
  return requestJson<AutomationEnvelope>('/api/v1/automations', {
    body: JSON.stringify(command),
    headers: { 'Content-Type': 'application/json' },
    method: 'POST',
    signal,
  }).then(unwrapAutomation)
}

export function fetchAutomation(
  automationId: string,
  signal?: AbortSignal,
): Promise<Automation> {
  return requestJson<AutomationEnvelope>(
    `/api/v1/automations/${encodeURIComponent(automationId)}`,
    { signal },
  ).then(unwrapAutomation)
}

export function updateAutomation(
  automationId: string,
  command: UpdateAutomationInput,
  signal?: AbortSignal,
): Promise<Automation> {
  return requestJson<AutomationEnvelope>(
    `/api/v1/automations/${encodeURIComponent(automationId)}`,
    {
      body: JSON.stringify(command),
      headers: { 'Content-Type': 'application/json' },
      method: 'PUT',
      signal,
    },
  ).then(unwrapAutomation)
}

export function updateAutomationPlacement(
  automationId: string,
  command: UpdateAutomationPlacementInput,
  signal?: AbortSignal,
): Promise<Automation> {
  return requestJson<AutomationEnvelope>(
    `/api/v1/automations/${encodeURIComponent(automationId)}/placement`,
    {
      body: JSON.stringify(command),
      headers: { 'Content-Type': 'application/json' },
      method: 'PUT',
      signal,
    },
  ).then(unwrapAutomation)
}

export function updateAutomationState(
  automationId: string,
  command: UpdateAutomationStateInput,
  signal?: AbortSignal,
): Promise<Automation> {
  return requestJson<AutomationEnvelope>(
    `/api/v1/automations/${encodeURIComponent(automationId)}/state`,
    {
      body: JSON.stringify(command),
      headers: { 'Content-Type': 'application/json' },
      method: 'PUT',
      signal,
    },
  ).then(unwrapAutomation)
}

export function fetchAutomationRuns(
  automationId: string,
  signal?: AbortSignal,
): Promise<AutomationRuns> {
  return requestJson(
    `/api/v1/automations/${encodeURIComponent(automationId)}/runs`,
    { signal },
  )
}

export function runAutomation(
  automationId: string,
  command: RunAutomationInput,
  signal?: AbortSignal,
): Promise<AutomationRun> {
  return requestJson<AutomationRunEnvelope>(
    `/api/v1/automations/${encodeURIComponent(automationId)}/runs`,
    {
      body: JSON.stringify(command),
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal,
    },
  ).then(unwrapAutomationRun)
}

export function createCoordinationNode(
  command: CreateCoordinationNodeInput,
  signal?: AbortSignal,
): Promise<CoordinationNodeCommandResult> {
  return requestJson('/api/v1/coordination-nodes', {
    body: JSON.stringify(command),
    headers: { 'Content-Type': 'application/json' },
    method: 'POST',
    signal,
  })
}

export function updateCoordinationNode(
  nodeId: string,
  command: UpdateCoordinationNodeInput,
  signal?: AbortSignal,
): Promise<CoordinationNodeCommandResult> {
  return requestJson(
    `/api/v1/coordination-nodes/${encodeURIComponent(nodeId)}`,
    {
      body: JSON.stringify(command),
      headers: { 'Content-Type': 'application/json' },
      method: 'PUT',
      signal,
    },
  )
}

export function updateCoordinationNodePlacement(
  nodeId: string,
  command: UpdateCoordinationNodePlacementInput,
  signal?: AbortSignal,
): Promise<CoordinationNodeCommandResult> {
  return requestJson(
    `/api/v1/coordination-nodes/${encodeURIComponent(nodeId)}/placement`,
    {
      body: JSON.stringify(command),
      headers: { 'Content-Type': 'application/json' },
      method: 'PUT',
      signal,
    },
  )
}

export function provisionCoordinationNode(
  nodeId: string,
  command: ProvisionCoordinationNodeInput,
  signal?: AbortSignal,
): Promise<CoordinationNodeCommandResult> {
  return requestJson(
    `/api/v1/coordination-nodes/${encodeURIComponent(nodeId)}/provision`,
    {
      body: JSON.stringify(command),
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal,
    },
  )
}

export function sendCoordinationNodePrompt(
  nodeId: string,
  command: SendCoordinationNodePromptInput,
  signal?: AbortSignal,
): Promise<CoordinationNodePromptAcknowledgement> {
  return requestJson(
    `/api/v1/coordination-nodes/${encodeURIComponent(nodeId)}/prompts`,
    {
      body: JSON.stringify(command),
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal,
    },
  )
}

export function fetchCoordinationNodeTerminalOutput(
  nodeId: string,
  lines = 120,
  signal?: AbortSignal,
): Promise<CoordinationNodeTerminalOutput> {
  return requestJson(
    `/api/v1/coordination-nodes/${encodeURIComponent(nodeId)}/terminal-output?lines=${lines}`,
    { signal },
  )
}

export function fetchCoordinationNodeRoutes(
  nodeId: string,
  limit = 100,
  signal?: AbortSignal,
): Promise<CoordinationNodeRoutes> {
  return requestJson(
    `/api/v1/coordination-nodes/${encodeURIComponent(nodeId)}/routes?limit=${limit}`,
    { signal },
  )
}

export function sendCoordinationNodeRoute(
  nodeId: string,
  command: SendCoordinationNodeRouteInput,
  signal?: AbortSignal,
): Promise<CoordinationNodeRoute> {
  return requestJson(
    `/api/v1/coordination-nodes/${encodeURIComponent(nodeId)}/routes`,
    {
      body: JSON.stringify(command),
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal,
    },
  )
}

export function fetchCoordinationSnapshots(
  nodeId: string,
  signal?: AbortSignal,
): Promise<CoordinationSnapshots> {
  return requestJson(
    `/api/v1/coordination-nodes/${encodeURIComponent(nodeId)}/snapshots`,
    { signal },
  )
}

export function requestCoordinationSnapshot(
  nodeId: string,
  command: RequestCoordinationSnapshotInput,
  signal?: AbortSignal,
): Promise<CoordinationSnapshot> {
  return requestJson(
    `/api/v1/coordination-nodes/${encodeURIComponent(nodeId)}/snapshots`,
    {
      body: JSON.stringify(command),
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal,
    },
  )
}

export function createProjectRelationship(
  command: CreateProjectRelationshipInput,
  signal?: AbortSignal,
): Promise<CreatedProjectRelationship> {
  return requestJson('/api/v1/project-relationships', {
    body: JSON.stringify(command),
    headers: { 'Content-Type': 'application/json' },
    method: 'POST',
    signal,
  })
}

export function deleteProjectRelationship(
  relationshipId: string,
  command: DeleteProjectRelationshipInput,
  signal?: AbortSignal,
): Promise<DeletedProjectRelationship> {
  return requestJson(
    `/api/v1/project-relationships/${encodeURIComponent(relationshipId)}/delete`,
    {
      body: JSON.stringify(command),
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal,
    },
  )
}

export function fetchYardOrchestrator(
  signal?: AbortSignal,
): Promise<YardOrchestrator> {
  return requestJson('/api/v1/yard/orchestrator', { signal })
}

export function configureYardOrchestrator(
  command: ConfigureYardOrchestratorInput,
  signal?: AbortSignal,
): Promise<ConfiguredYardOrchestrator> {
  return requestJson('/api/v1/yard/orchestrator', {
    body: JSON.stringify(command),
    headers: { 'Content-Type': 'application/json' },
    method: 'PUT',
    signal,
  })
}

export function provisionYardOrchestrator(
  command: ProvisionYardOrchestratorInput,
  signal?: AbortSignal,
): Promise<ConfiguredYardOrchestrator> {
  return requestJson('/api/v1/yard/orchestrator', {
    body: JSON.stringify(command),
    headers: { 'Content-Type': 'application/json' },
    method: 'POST',
    signal,
  })
}

export function recoverYardOrchestrator(
  command: RecoverYardOrchestratorInput,
  signal?: AbortSignal,
): Promise<RecoveredYardOrchestrator> {
  return requestJson('/api/v1/yard/orchestrator/recover', {
    body: JSON.stringify(command),
    headers: { 'Content-Type': 'application/json' },
    method: 'POST',
    signal,
  })
}

export function createProject(
  project: CreateProjectInput,
  signal?: AbortSignal,
): Promise<Project> {
  return requestJson('/api/v1/projects', {
    body: JSON.stringify(project),
    headers: { 'Content-Type': 'application/json' },
    method: 'POST',
    signal,
  })
}

export function createProjectFromProfile(
  command: CreateProjectFromProfileInput,
  signal?: AbortSignal,
): Promise<ConfirmedProjectCreation> {
  return requestJson('/api/v1/projects/from-profile', {
    body: JSON.stringify(command),
    headers: { 'Content-Type': 'application/json' },
    method: 'POST',
    signal,
  })
}

export function createProjectWithWorkspace(
  command: CreateWorkspaceProjectFromProfileInput,
  signal?: AbortSignal,
): Promise<ConfirmedProjectCreation> {
  return requestJson('/api/v1/projects/from-profile/workspace', {
    body: JSON.stringify(command),
    headers: { 'Content-Type': 'application/json' },
    method: 'POST',
    signal,
  })
}

export function updateProjectPlacement(
  projectId: string,
  update: UpdateProjectPlacementInput,
  signal?: AbortSignal,
): Promise<Project> {
  return requestJson(
    `/api/v1/projects/${encodeURIComponent(projectId)}/placement`,
    {
      body: JSON.stringify(update),
      headers: { 'Content-Type': 'application/json' },
      method: 'PUT',
      signal,
    },
  )
}

export function fetchWorkerProfiles(
  signal?: AbortSignal,
): Promise<WorkerProfiles> {
  return requestJson('/api/v1/worker-profiles', { signal })
}

export function fetchWorkers(signal?: AbortSignal): Promise<WorkerCandidates> {
  return requestJson('/api/v1/workers', { signal })
}

export function fetchCompletedRuntimeCleanupPreview(
  signal?: AbortSignal,
  limit = 50,
): Promise<CompletedRuntimeCleanupPreview> {
  return requestJson(
    `/api/v1/workers/completed-runtime-cleanup-preview?limit=${limit}`,
    { signal },
  )
}

export function endWorkerSession(
  workerId: string,
  command: EndWorkerSessionInput,
  signal?: AbortSignal,
): Promise<EndedWorkerSession> {
  return requestJson(
    `/api/v1/workers/${encodeURIComponent(workerId)}/end-session`,
    {
      body: JSON.stringify(command),
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal,
    },
  )
}

export function deleteWorker(
  workerId: string,
  command: DeleteWorkerInput,
  signal?: AbortSignal,
): Promise<DeletedWorker> {
  return requestJson(
    `/api/v1/workers/${encodeURIComponent(workerId)}/delete`,
    {
      body: JSON.stringify(command),
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal,
    },
  )
}

export function createWorkerProfile(
  profile: CreateWorkerProfileInput,
  signal?: AbortSignal,
): Promise<WorkerProfile> {
  return requestJson('/api/v1/worker-profiles', {
    body: JSON.stringify(profile),
    headers: { 'Content-Type': 'application/json' },
    method: 'POST',
    signal,
  })
}

export function updateWorkerProfile(
  profileId: string,
  profile: UpdateWorkerProfileInput,
  signal?: AbortSignal,
): Promise<WorkerProfile> {
  return requestJson(
    `/api/v1/worker-profiles/${encodeURIComponent(profileId)}`,
    {
      body: JSON.stringify(profile),
      headers: { 'Content-Type': 'application/json' },
      method: 'PUT',
      signal,
    },
  )
}

export function fetchProjectAssignments(
  projectId: string,
  signal?: AbortSignal,
): Promise<Assignments> {
  return requestJson(
    `/api/v1/projects/${encodeURIComponent(projectId)}/assignments`,
    { signal },
  )
}

export function confirmWorkerAssignment(
  projectId: string,
  command: ConfirmWorkerAssignmentInput,
  signal?: AbortSignal,
): Promise<ConfirmedAllocation> {
  return requestJson(
    `/api/v1/projects/${encodeURIComponent(projectId)}/assignments`,
    {
      body: JSON.stringify(command),
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal,
    },
  )
}

export function confirmWorkerHandoff(
  sourceProjectId: string,
  sourceAssignmentId: string,
  command: ConfirmWorkerHandoffInput,
  signal?: AbortSignal,
): Promise<ConfirmedWorkerHandoff> {
  return requestJson(
    `/api/v1/projects/${encodeURIComponent(sourceProjectId)}/assignments/${encodeURIComponent(sourceAssignmentId)}/handoffs`,
    {
      body: JSON.stringify(command),
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal,
    },
  )
}

export function recordCompletionReceipt(
  projectId: string,
  assignmentId: string,
  command: RecordCompletionReceiptInput,
  signal?: AbortSignal,
): Promise<RecordedCompletionReceipt> {
  return requestJson(
    `/api/v1/projects/${encodeURIComponent(projectId)}/assignments/${encodeURIComponent(assignmentId)}/completion-receipts`,
    {
      body: JSON.stringify(command),
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal,
    },
  )
}

export function uploadArtifact(
  projectId: string,
  assignmentId: string,
  artifactId: string,
  artifact: UploadArtifactInput,
  signal?: AbortSignal,
): Promise<Artifact> {
  return requestJson(
    `/api/v1/projects/${encodeURIComponent(projectId)}/assignments/${encodeURIComponent(assignmentId)}/artifacts/${encodeURIComponent(artifactId)}`,
    {
      body: JSON.stringify(artifact),
      headers: { 'Content-Type': 'application/json' },
      method: 'PUT',
      signal,
    },
  )
}

export function fetchArtifactContent(
  projectId: string,
  assignmentId: string,
  artifactId: string,
  signal?: AbortSignal,
): Promise<ArtifactContent> {
  return requestJson(
    `/api/v1/projects/${encodeURIComponent(projectId)}/assignments/${encodeURIComponent(assignmentId)}/artifacts/${encodeURIComponent(artifactId)}/content`,
    { signal },
  )
}

export function sendAssignmentPrompt(
  projectId: string,
  assignmentId: string,
  command: SendAssignmentPromptInput,
  signal?: AbortSignal,
): Promise<PromptAcknowledgement> {
  return requestJson(
    `/api/v1/projects/${encodeURIComponent(projectId)}/assignments/${encodeURIComponent(assignmentId)}/prompts`,
    {
      body: JSON.stringify(command),
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal,
    },
  )
}

export function fetchAssignmentTerminalOutput(
  projectId: string,
  assignmentId: string,
  lines = 120,
  signal?: AbortSignal,
): Promise<TerminalOutput> {
  return requestJson(
    `/api/v1/projects/${encodeURIComponent(projectId)}/assignments/${encodeURIComponent(assignmentId)}/terminal-output?lines=${lines}`,
    { signal },
  )
}

export function sendOrchestratorPrompt(
  projectId: string,
  command: SendOrchestratorPromptInput,
  signal?: AbortSignal,
): Promise<OrchestratorPromptAcknowledgement> {
  return requestJson(
    `/api/v1/projects/${encodeURIComponent(projectId)}/orchestrator/prompts`,
    {
      body: JSON.stringify(command),
      headers: { 'Content-Type': 'application/json' },
      method: 'POST',
      signal,
    },
  )
}

export function fetchOrchestratorTerminalOutput(
  projectId: string,
  lines = 120,
  signal?: AbortSignal,
): Promise<OrchestratorTerminalOutput> {
  return requestJson(
    `/api/v1/projects/${encodeURIComponent(projectId)}/orchestrator/terminal-output?lines=${lines}`,
    { signal },
  )
}

export function fetchOrchestratorStatusOutput(
  projectId: string,
  signal?: AbortSignal,
): Promise<OrchestratorTerminalOutput> {
  return fetchOrchestratorTerminalOutput(projectId, 80, signal)
}

export function sendYardOrchestratorPrompt(
  command: SendYardOrchestratorPromptInput,
  signal?: AbortSignal,
): Promise<YardOrchestratorPromptAcknowledgement> {
  return requestJson('/api/v1/yard/orchestrator/prompts', {
    body: JSON.stringify(command),
    headers: { 'Content-Type': 'application/json' },
    method: 'POST',
    signal,
  })
}

export function fetchYardOrchestratorRoutes(
  limit = 100,
  signal?: AbortSignal,
): Promise<YardOrchestratorRoutes> {
  return requestJson(
    `/api/v1/yard/orchestrator/routes?limit=${limit}`,
    { signal },
  )
}

export function sendYardOrchestratorRoute(
  command: SendYardOrchestratorRouteInput,
  signal?: AbortSignal,
): Promise<YardOrchestratorRoute> {
  return requestJson('/api/v1/yard/orchestrator/routes', {
    body: JSON.stringify(command),
    headers: { 'Content-Type': 'application/json' },
    method: 'POST',
    signal,
  })
}

export function fetchYardOrchestratorTerminalOutput(
  lines = 120,
  signal?: AbortSignal,
): Promise<YardOrchestratorTerminalOutput> {
  return requestJson(
    `/api/v1/yard/orchestrator/terminal-output?lines=${lines}`,
    { signal },
  )
}

export function openTerminalInGhostty(
  session: string,
  terminalId: string,
  signal?: AbortSignal,
): Promise<ExternalTerminalLaunch> {
  return requestJson(
    `/api/v1/runtimes/herdr/sessions/${encodeURIComponent(session)}/terminals/${encodeURIComponent(terminalId)}/open-ghostty`,
    { method: 'POST', signal },
  )
}

function terminalWebSocketUrl(
  path: string,
  cols: number,
  rows: number,
  pageLocation: Pick<Location, 'href' | 'protocol'>,
): string {
  const url = new URL(path, pageLocation.href)
  url.protocol = pageLocation.protocol === 'https:' ? 'wss:' : 'ws:'
  url.searchParams.set('cols', String(cols))
  url.searchParams.set('rows', String(rows))
  return url.toString()
}

export function assignmentTerminalWebSocketUrl(
  projectId: string,
  assignmentId: string,
  cols: number,
  rows: number,
  pageLocation: Pick<Location, 'href' | 'protocol'> = window.location,
): string {
  return terminalWebSocketUrl(
    `/api/v1/projects/${encodeURIComponent(projectId)}/assignments/${encodeURIComponent(assignmentId)}/terminal`,
    cols,
    rows,
    pageLocation,
  )
}

export function orchestratorTerminalWebSocketUrl(
  projectId: string,
  cols: number,
  rows: number,
  pageLocation: Pick<Location, 'href' | 'protocol'> = window.location,
): string {
  return terminalWebSocketUrl(
    `/api/v1/projects/${encodeURIComponent(projectId)}/orchestrator/terminal`,
    cols,
    rows,
    pageLocation,
  )
}

export function yardOrchestratorTerminalWebSocketUrl(
  cols: number,
  rows: number,
  pageLocation: Pick<Location, 'href' | 'protocol'> = window.location,
): string {
  return terminalWebSocketUrl(
    '/api/v1/yard/orchestrator/terminal',
    cols,
    rows,
    pageLocation,
  )
}

export function coordinationNodeTerminalWebSocketUrl(
  nodeId: string,
  cols: number,
  rows: number,
  pageLocation: Pick<Location, 'href' | 'protocol'> = window.location,
): string {
  return terminalWebSocketUrl(
    `/api/v1/coordination-nodes/${encodeURIComponent(nodeId)}/terminal`,
    cols,
    rows,
    pageLocation,
  )
}

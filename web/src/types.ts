export type ObservedStatus =
  | 'idle'
  | 'working'
  | 'blocked'
  | 'done'
  | 'unknown'

export type WorkflowStatus = 'working' | 'needs_attention' | 'idle'

export interface StatusReport {
  version: 1
  command_id: string
  state: WorkflowStatus
  last: string
  next: string
  blockers: string[]
}

export type RuntimeObservationState = 'observed' | 'missing' | 'ambiguous'
export type RuntimeProcessState = 'running' | 'exited' | 'unknown'

export interface RuntimeSession {
  name: string
  is_default: boolean
  running: boolean
}

export interface RuntimeSessions {
  adapter: string
  sessions: RuntimeSession[]
}

export type HerdrPaneKind = 'agent' | 'runtime'
export type HerdrPaneObservation = 'observed' | 'unobserved' | 'ambiguous'

export interface HerdrLivePane {
  identity_key: string
  kind: HerdrPaneKind
  observation: HerdrPaneObservation
  reason: string
  metadata_reason: string | null
  has_live_pane: boolean
  session: string
  workspace_id: string | null
  workspace_label: string | null
  tab_id: string | null
  pane_id: string | null
  pane_instance_id?: string | null
  terminal_id: string | null
  label: string | null
  name: string | null
  provider: string | null
  display_provider: string | null
  status: ObservedStatus
  cwd: string | null
  foreground_cwd: string | null
}

export interface HerdrLiveSession {
  id: string
  name: string
  is_default: boolean
  metadata_ambiguous: boolean
  metadata_reason: string | null
  observed_at_unix_ms: string
  pane_count: number
  panes: HerdrLivePane[]
}

export interface HerdrSessionFailure {
  session: string
  code: string
  reason: string
}

export interface HerdrFleetInventory {
  adapter: string
  live_pane_count: number
  sessions: HerdrLiveSession[]
  failures: HerdrSessionFailure[]
}

export interface FocusObservation {
  workspace_id: string | null
  tab_id: string | null
  pane_id: string | null
}

export interface WorktreeObservation {
  repository_key: string
  repository_name: string
  repository_root: string
  checkout_path: string
  is_linked: boolean
}

export interface WorkspaceObservation {
  runtime_id: string
  order: number
  label: string
  focused: boolean
  active_tab_id: string
  pane_count: number
  tab_count: number
  status: ObservedStatus
  tokens: Record<string, string>
  worktree: WorktreeObservation | null
}

export interface TabObservation {
  runtime_id: string
  workspace_id: string
  order: number
  label: string
  focused: boolean
  pane_count: number
  status: ObservedStatus
}

export interface ProviderSessionRef {
  source: string
  provider: string
  kind: string
  value: string
}

export interface PaneObservation {
  runtime_id: string
  pane_instance_id?: string | null
  terminal_id: string
  workspace_id: string
  tab_id: string
  focused: boolean
  cwd: string | null
  foreground_cwd: string | null
  label: string | null
  provider: string | null
  display_provider: string | null
  status: ObservedStatus
  tokens: Record<string, string>
  provider_session: ProviderSessionRef | null
  revision: string
}

export interface ObservedWorker {
  runtime_id: string
  pane_instance_id?: string | null
  terminal_id: string
  workspace_id: string
  tab_id: string
  pane_id: string
  name: string | null
  provider: string | null
  display_provider: string | null
  status: ObservedStatus
  focused: boolean
  launch_pending: boolean
  interactive_ready: boolean
  state_change_sequence: string
  cwd: string | null
  foreground_cwd: string | null
  tokens: Record<string, string>
  provider_session: ProviderSessionRef | null
  revision: string
}

export type PaneManagementCategory =
  | 'eligible'
  | 'already_managed'
  | 'conflict'
  | 'ambiguous'
  | 'unsupported'

export interface PaneManagementCandidate {
  candidate_key: string
  category: PaneManagementCategory
  reason: string
  session: string
  workspace_id: string | null
  workspace_label: string | null
  pane_id: string | null
  pane_instance_id: string | null
  terminal_id: string | null
  project_id: string | null
  project_name: string | null
  worker_id: string | null
  provider: string | null
  display_provider: string | null
  recovery_required: boolean
  management_controls_enabled: boolean
}

export interface PaneManagementPreview {
  supported: boolean
  endpoint: string | null
  limit: number
  truncated: boolean
  candidate_count: number
  eligible_count: number
  candidates: PaneManagementCandidate[]
}

export interface PaneManagementBatchResult {
  command_id: string
  replayed: boolean
  managed_count: number
  results: Array<{
    candidate_key: string
    outcome:
      | 'managed'
      | 'already_managed'
      | 'conflict'
      | 'skipped'
      | 'failed'
      | 'rollback_failed'
    reason: string
    worker_id: string | null
  }>
}

export interface ManageAllAgentsInput {
  command_id: string
  actor: string
  confirmed: boolean
  candidate_keys: string[]
}

export interface ObservedChildAgent {
  runtime_id: string
  parent_provider_session: ProviderSessionRef
  parent_agent_id: string | null
  provider: string
  provider_agent_id: string
  name: string | null
  description: string | null
  role: string | null
  status: ObservedStatus
  depth: number
  updated_at_unix_ms: number
}

export interface RuntimeInventory {
  adapter: string
  session: string
  runtime_version: string
  protocol: number
  observed_at_unix_ms: number
  focus: FocusObservation
  workspaces: WorkspaceObservation[]
  tabs: TabObservation[]
  panes: PaneObservation[]
  workers: ObservedWorker[]
  child_agents: ObservedChildAgent[]
}

export type ManagedRuntimeWorkspaceKind =
  | 'yard_central'
  | 'coordination'
  | 'provisioning'
  | 'quarantined'
  | 'cleanup_pending'

export interface ManagedRuntimeOccupant {
  kind: 'provisioning' | 'quarantined' | 'cleanup_pending'
  terminal_id: string
  tab_id: string | null
  pane_id: string
  label: string
  reason: string
  project_name: string | null
}

export interface ManagedRuntimeWorkspace {
  workspace_id: string
  kind: ManagedRuntimeWorkspaceKind
  label: string
  occupants: ManagedRuntimeOccupant[]
}

export interface RuntimeTopology {
  adapter: string
  session: string
  managed_workspaces: ManagedRuntimeWorkspace[]
}

export type RuntimeLensClassification =
  | 'linked_yard_worker'
  | 'unassigned_herdr_agent'
  | 'topology_only_shell_pane'
  | 'stale_missing_binding'
  | 'ambiguous_identity'

export interface RuntimeLensEntry {
  classification: RuntimeLensClassification
  session: string
  workspace_id: string
  tab_id: string | null
  pane_id: string
  terminal_id: string
  observed_at_unix_ms: string
  binding_last_observed_at_unix_ms: string | null
  snapshot_current: boolean
  reason: string
  worker_id: string | null
  profile_name: string | null
  availability: WorkerAvailability | null
  provider: string | null
  display_provider: string | null
  name: string | null
  label: string | null
  status: ObservedStatus
  focused: boolean
  launch_pending: boolean
  interactive_ready: boolean
}

export interface RuntimeLens {
  adapter: string
  selected_session: string
  snapshot_current: boolean
  observed_at_unix_ms: string
  inventory: RuntimeInventory
  topology: RuntimeTopology
  workers: WorkerCandidates
  entries: RuntimeLensEntry[]
}

export interface CanvasPlacement {
  x: number
  y: number
  width: number
  height: number
}

export interface ProjectRuntimeBinding {
  adapter: string
  session: string
  workspace_id: string
}

export interface WorkerRuntimeBinding extends ProjectRuntimeBinding {
  terminal_id: string
  tab_id: string | null
  pane_id: string
  provider_session: ProviderSessionRef | null
  owns_tab: boolean
  observation_state: RuntimeObservationState
  process_state: RuntimeProcessState
  status: ObservedStatus
  state_change_sequence: string
  revision: string
  version: string
  last_observed_at_unix_ms: number
}

export interface Worker {
  id: string
  profile_id: string | null
  profile_version: string | null
  desired_state: 'running' | 'ended'
  runtime: WorkerRuntimeBinding | null
  version: string
  created_at_unix_ms: number
  updated_at_unix_ms: number
}

export interface ProjectPlacement {
  geometry: CanvasPlacement
  version: string
  updated_at_unix_ms: number
}

export interface ProjectWorkflowProfilePin {
  profile_id: string
  profile_version: string
  pinned_by: string
  pinned_at_unix_ms: number
}

export interface Project {
  id: string
  name: string
  runtime: ProjectRuntimeBinding
  orchestrator: Worker
  placement: ProjectPlacement
  workflow_profile: ProjectWorkflowProfilePin
  version: string
  created_at_unix_ms: number
  updated_at_unix_ms: number
}

export interface YardOrchestrator {
  worker: Worker | null
  version: string
  workflow_profile_version: string
  created_at_unix_ms: number
  updated_at_unix_ms: number
}

export interface ConfigureYardOrchestratorInput {
  command_id: string
  actor: string
  worker_id: string
  expected_worker_version: string
  expected_orchestrator_version: string
  workflow_profile_version?: string
}

export interface ProvisionYardOrchestratorInput {
  command_id: string
  actor: string
  profile_id: string
  expected_profile_version: string
  expected_orchestrator_version: string
}

export interface RecoverYardOrchestratorInput {
  command_id: string
  actor: string
  expected_orchestrator_version: string
}

export interface ConfiguredYardOrchestrator {
  command_id: string
  orchestrator: YardOrchestrator
  replaced_worker_id: string | null
  replayed: boolean
}

export interface RecoveredYardOrchestrator {
  command_id: string
  orchestrator: YardOrchestrator
}

export interface Projects {
  projects: Project[]
}

export interface ArchiveProjectInput {
  command_id: string
  actor: string
  expected_project_version: string
  expected_orchestrator_worker_id: string
  expected_orchestrator_worker_version: string
  expected_orchestrator_runtime_version: string | null
}

export interface ArchivedProject {
  command_id: string
  project_id: string
  orchestrator_worker_id: string
  archived_at_unix_ms: number
  cleanup_pending: boolean
  replayed: boolean
}

export interface DeleteProjectInput {
  command_id: string
  actor: string
}

export interface DeletedProject {
  command_id: string
  project_id: string
  orchestrator_worker_id: string
  deleted_at_unix_ms: number
  cleanup_pending: boolean
  replayed: boolean
}

export interface ChangeProjectOrchestratorInput {
  command_id: string
  actor: string
  worker_id: string
  expected_worker_version: string
  expected_worker_runtime: WorkerRuntimeBinding
  expected_project_version: string
  expected_orchestrator_worker_id: string
  expected_orchestrator_worker_version: string
  expected_orchestrator_runtime: WorkerRuntimeBinding
}

export interface ChangedProjectOrchestrator {
  command_id: string
  project: Project
  replaced_worker_id: string
  replayed: boolean
}

export type ProjectRelationshipKind = 'depends_on'

export interface ProjectRelationship {
  id: string
  source_project_id: string
  target_project_id: string
  kind: ProjectRelationshipKind
  version: string
  created_by: string
  created_at_unix_ms: number
  updated_at_unix_ms: number
}

export interface ProjectRelationships {
  relationships: ProjectRelationship[]
}

export interface CreateProjectRelationshipInput {
  command_id: string
  actor: string
  relationship_id: string
  source_project_id: string
  target_project_id: string
  kind: ProjectRelationshipKind
}

export interface CreatedProjectRelationship {
  command_id: string
  relationship: ProjectRelationship
  replayed: boolean
}

export interface DeleteProjectRelationshipInput {
  command_id: string
  actor: string
  expected_version: string
}

export interface DeletedProjectRelationship {
  command_id: string
  relationship_id: string
  deleted_at_unix_ms: number
  replayed: boolean
}

export interface CreateProjectInput {
  name: string
  runtime: ProjectRuntimeBinding
  orchestrator_observed_worker_id: string
  placement: CanvasPlacement
}

export interface CreateProjectFromProfileInput {
  command_id: string
  actor: string
  name: string
  runtime: ProjectRuntimeBinding
  profile_id: string
  expected_profile_version: string
  orchestrator_objective: string
  placement: CanvasPlacement
}

export interface CreateWorkspaceProjectFromProfileInput {
  command_id: string
  actor: string
  name: string
  runtime_adapter: string
  runtime_session: string
  workspace_label: string
  cwd: string
  profile_id: string
  expected_profile_version: string
  orchestrator_objective: string
  placement: CanvasPlacement
}

export interface ConfirmedProjectCreation {
  command_id: string
  project: Project
  replayed: boolean
}

export interface UpdateProjectPlacementInput {
  placement: CanvasPlacement
  expected_version: string
}

export interface WorkerProfileSpec {
  name: string
  runtime_adapter: string
  provider: string
  model: string | null
  default_role: string
  instructions_ref: string | null
  tools: string[]
  skills: string[]
  mcp_servers: string[]
  sandbox_policy: string
  worktree_policy: string
  permission_policy: string
  completion_contract: string
}

export interface WorkerProfile extends WorkerProfileSpec {
  id: string
  version: string
  created_at_unix_ms: number
  updated_at_unix_ms: number
}

export interface WorkerProfiles {
  profiles: WorkerProfile[]
}

export interface CreateWorkerProfileInput extends WorkerProfileSpec {}

export interface UpdateWorkerProfileInput extends WorkerProfileSpec {
  expected_version: string
}

export type AssignmentLifecycle =
  | 'allocating'
  | 'active'
  | 'handing_off'
  | 'handed_off'
  | 'completed'
  | 'failed'
export type AttemptLifecycle =
  | 'starting'
  | 'active'
  | 'handing_off'
  | 'handed_off'
  | 'completed'
  | 'failed'
export type HandoffTargetRole = 'member' | 'orchestrator'

export interface AssignmentAttempt {
  id: string
  assignment_id: string
  ordinal: number
  lifecycle: AttemptLifecycle
  error: string | null
  version: string
  created_at_unix_ms: number
  updated_at_unix_ms: number
}

export interface CompletionReceipt {
  id: string
  assignment_id: string
  attempt_id: string
  outcome: 'completed'
  summary: string
  artifact_refs: string[]
  artifacts: Artifact[]
  evidence_refs: string[]
  unresolved_blockers: string[]
  actor: string
  created_at_unix_ms: number
}

export type ArtifactKind = 'markdown' | 'html'

export interface Artifact {
  id: string
  project_id: string
  assignment_id: string
  attempt_id: string
  worker_id: string
  kind: ArtifactKind
  media_type: 'text/markdown' | 'text/html'
  display_name: string
  byte_size: number
  sha256: string
  source: 'upload'
  created_by: string
  created_at_unix_ms: number
}

export interface ArtifactContent {
  artifact: Artifact
  content: string
}

export interface Assignment {
  id: string
  project_id: string
  allocation_id: string
  worker: Worker
  profile_id: string
  profile_version: string
  profile_name: string
  objective: string
  role: string
  isolation_policy: 'project_workspace'
  lifecycle: AssignmentLifecycle
  attempt: AssignmentAttempt
  completion_receipt: CompletionReceipt | null
  version: string
  created_at_unix_ms: number
  updated_at_unix_ms: number
}

export interface Assignments {
  assignments: Assignment[]
}

export type WorkerAvailability =
  | 'yard_orchestrator'
  | 'orchestrator'
  | 'assigned'
  | 'unassigned_live'
  | 'resumable'
  | 'unavailable'
  | 'ambiguous'
  | 'ended'

export interface WorkerCandidate {
  worker: Worker
  profile_name?: string | null
  default_role?: string | null
  availability: WorkerAvailability
  project_id?: string | null
  assignment_id?: string | null
  reason?: string | null
}

export interface WorkerCandidates {
  workers: WorkerCandidate[]
}

export type CompletedRuntimeRetentionReason =
  | 'cleanup_policy_unavailable'
  | 'approval_authority_unavailable'
  | 'terminal_lease_fence_unavailable'
  | 'observation_generation_fence_unavailable'
  | 'atomic_close_unavailable'
  | 'grace_policy_unavailable'
  | 'no_linked_artifacts'
  | 'unresolved_completion_blockers'
  | 'pending_assignment_intervention'
  | 'new_active_assignment'
  | 'protected_orchestrator'

export interface CompletedRuntimeCleanupCandidate {
  worker_id: string
  profile_name: string
  project_id: string
  project_name: string
  assignment_id: string
  role: string
  completion_receipt_id: string
  completed_at_unix_ms: number
  linked_artifact_count: number
  close_eligible: false
  retained_reasons: CompletedRuntimeRetentionReason[]
}

export interface CompletedRuntimeCleanupPreview {
  candidate_count: number
  close_ready_count: 0
  limit: number
  truncated: boolean
  candidates: CompletedRuntimeCleanupCandidate[]
}

export interface EndWorkerSessionInput {
  command_id: string
  actor: string
  expected_worker_version: string
  expected_runtime_version?: string
}

export interface EndedWorkerSession {
  command_id: string
  worker: Worker
  cleanup_pending: boolean
  replayed: boolean
}

export interface DeleteWorkerInput {
  command_id: string
  actor: string
  expected_worker_version: string
}

export interface DeletedWorker {
  command_id: string
  worker_id: string
  deleted_at_unix_ms: number
  cleanup_pending: boolean
  replayed: boolean
}

interface ConfirmAllocationInput {
  command_id: string
  actor: string
  expected_project_version: string
  objective: string
  role: string
  isolation_policy: 'project_workspace'
}

export interface ConfirmProfileAllocationInput
  extends ConfirmAllocationInput {
  profile_id: string
  expected_profile_version: string
}

export interface ConfirmWorkerAllocationInput
  extends ConfirmAllocationInput {
  worker_id: string
  expected_worker_version: string
  profile_id?: string
  expected_profile_version?: string
}

export type ConfirmWorkerAssignmentInput =
  | ConfirmProfileAllocationInput
  | ConfirmWorkerAllocationInput

export interface WorkerAllocation {
  id: string
  project_id: string
  worker_id: string
  mode: string
  started_by_command_id: string | null
  started_at_unix_ms: number
}

export interface ConfirmedAllocation {
  command_id: string
  allocation: WorkerAllocation
  assignment: Assignment
  replayed: boolean
}

export interface ConfirmWorkerHandoffInput {
  command_id: string
  actor: string
  worker_id: string
  expected_worker_version: string
  expected_source_project_version: string
  target_project_id: string
  expected_target_project_version: string
  source_attempt_id: string
  expected_source_assignment_version: string
  expected_source_attempt_version: string
  target_role: HandoffTargetRole
  objective: string
  role: string
  isolation_policy: 'project_workspace'
}

export interface ConfirmedWorkerHandoff {
  command_id: string
  source_assignment: Assignment
  allocation: WorkerAllocation
  assignment: Assignment
  target_role: HandoffTargetRole
  replaced_orchestrator_worker_id: string | null
  replayed: boolean
}

export interface RecordCompletionReceiptInput {
  command_id: string
  actor: string
  attempt_id: string
  expected_assignment_version: string
  expected_attempt_version: string
  outcome: 'completed'
  summary: string
  artifact_refs: string[]
  artifact_ids: string[]
  evidence_refs: string[]
  unresolved_blockers: string[]
}

export interface UploadArtifactInput {
  actor: string
  attempt_id: string
  expected_assignment_version: string
  expected_attempt_version: string
  kind: ArtifactKind
  display_name: string
  content: string
}

export interface RecordedCompletionReceipt {
  command_id: string
  receipt: CompletionReceipt
  assignment: Assignment
  replayed: boolean
}

export interface SendAssignmentPromptInput {
  command_id: string
  actor: string
  attempt_id: string
  expected_assignment_version: string
  expected_attempt_version: string
  text: string
}

export interface SendOrchestratorPromptInput {
  command_id: string
  actor: string
  expected_project_version: string
  orchestrator_worker_id: string
  text: string
}

export interface SendYardOrchestratorPromptInput {
  command_id: string
  actor: string
  expected_orchestrator_version: string
  orchestrator_worker_id: string
  text: string
}

export interface SendYardOrchestratorRouteInput {
  command_id: string
  actor: string
  expected_orchestrator_version: string
  orchestrator_worker_id: string
  target_project_id: string
  expected_project_version: string
  target_orchestrator_worker_id: string
  text: string
}

export type CoordinationCommandStatus =
  | 'pending'
  | 'submitted'
  | 'failed'
  | 'ambiguous'

export interface YardOrchestratorRoute {
  command_id: string
  actor: string
  orchestrator_worker_id: string
  expected_orchestrator_version: string
  target_project_id: string
  target_orchestrator_worker_id: string
  expected_project_version: string
  text: string
  status: CoordinationCommandStatus
  error_message: string | null
  runtime_status: string | null
  created_at_unix_ms: number
  updated_at_unix_ms: number
  submitted_at_unix_ms: number | null
}

export interface YardOrchestratorRoutes {
  routes: YardOrchestratorRoute[]
}

export type CoordinationNodeKind = 'workstream' | 'knowledge_store'

export interface CoordinationNodePlacement {
  geometry: CanvasPlacement
  version: string
  updated_at_unix_ms: number
}

export interface CoordinationNode {
  id: string
  name: string
  kind: CoordinationNodeKind
  placement: CoordinationNodePlacement
  attached_project_ids: string[]
  worker: Worker | null
  cwd: string | null
  folder_path: string | null
  version: string
  created_by: string
  created_at_unix_ms: number
  updated_at_unix_ms: number
}

export interface CoordinationNodes {
  nodes: CoordinationNode[]
}

export interface CreateCoordinationNodeInput {
  command_id: string
  actor: string
  name: string
  kind: CoordinationNodeKind
  placement: CanvasPlacement
  attached_project_ids: string[]
}

export interface UpdateCoordinationNodeInput {
  command_id: string
  actor: string
  expected_version: string
  name: string
  attached_project_ids: string[]
}

export interface UpdateCoordinationNodePlacementInput {
  command_id: string
  actor: string
  expected_version: string
  placement: CanvasPlacement
}

export interface CoordinationNodeCommandResult {
  command_id: string
  node: CoordinationNode
  replayed: boolean
}

export interface ProvisionCoordinationNodeInput {
  command_id: string
  actor: string
  profile_id: string
  expected_profile_version: string
  expected_node_version: string
}

export interface SendCoordinationNodePromptInput {
  command_id: string
  actor: string
  expected_node_version: string
  worker_id: string
  text: string
}

export interface CoordinationNodePromptAcknowledgement {
  command_id: string
  node_id: string
  worker_id: string
  runtime_status: string
  submitted_at_unix_ms: number
}

export interface CoordinationNodeTerminalOutput {
  node_id: string
  worker_id: string
  pane_id: string
  source: string
  format: string
  text: string
  revision: string
  truncated: boolean
  status_report?: unknown
}

export interface SendCoordinationNodeRouteInput {
  command_id: string
  actor: string
  expected_node_version: string
  worker_id: string
  target_project_id: string
  expected_project_version: string
  target_orchestrator_worker_id: string
  text: string
}

export interface CoordinationNodeRoute {
  command_id: string
  node_id: string
  actor: string
  worker_id: string
  expected_node_version: string
  target_project_id: string
  target_orchestrator_worker_id: string
  expected_project_version: string
  text: string
  status: CoordinationCommandStatus
  error_message: string | null
  runtime_status: string | null
  created_at_unix_ms: number
  updated_at_unix_ms: number
  submitted_at_unix_ms: number | null
}

export interface CoordinationNodeRoutes {
  routes: CoordinationNodeRoute[]
}

export interface RequestCoordinationSnapshotInput {
  command_id: string
  actor: string
  expected_node_version: string
}

export type SnapshotCollectionStatus = 'pending' | 'collected'

export interface SnapshotProjectCollection {
  project_id: string
  project_version: string
  orchestrator_worker_id: string
  folder_path: string
  collection_status: SnapshotCollectionStatus
  delivery_status: CoordinationCommandStatus
  delivery_error: string | null
  runtime_status: string | null
  submitted_at_unix_ms: number | null
  collected_at_unix_ms: number | null
}

export interface CoordinationSnapshot {
  id: string
  node_id: string
  command_id: string
  folder_path: string
  projects: SnapshotProjectCollection[]
  progress: {
    completed: number
    total: number
  }
  requested_by: string
  created_at_unix_ms: number
  replayed: boolean
}

export interface CoordinationSnapshots {
  snapshots: CoordinationSnapshot[]
}

export type AutomationScope =
  | { kind: 'yard_orchestrator' }
  | { kind: 'project_orchestrator'; project_id: string }
  | { kind: 'workstream_coordination_node'; node_id: string }

export interface AutomationPlacement {
  geometry: CanvasPlacement
  version: string
  updated_at_unix_ms: number
}

export interface AutomationSchedule {
  hour: number
  minute: number
  timezone: string
}

export type AutomationState = 'active' | 'paused'
export type AutomationRunTrigger = 'manual' | 'scheduled'
export type AutomationRunStatus =
  | 'pending'
  | 'submitted'
  | 'failed'
  | 'ambiguous'

export interface AutomationRun {
  id: string
  automation_id: string
  automation_version: string
  trigger: AutomationRunTrigger
  scheduled_for_unix_ms: number | null
  dispatch_command_id: string
  prompt_template: string
  selected_project_ids: string[]
  status: AutomationRunStatus
  runtime_status: string | null
  error_message: string | null
  submitted_at_unix_ms: number | null
  requested_by: string
  version: string
  created_at_unix_ms: number
  updated_at_unix_ms: number
}

export interface Automation {
  id: string
  name: string
  scope: AutomationScope
  placement: AutomationPlacement
  schedule: AutomationSchedule
  selected_project_ids: string[]
  prompt_template: string
  state: AutomationState
  next_run_at_unix_ms: number | null
  latest_run: AutomationRun | null
  version: string
  created_by: string
  created_at_unix_ms: number
  updated_at_unix_ms: number
}

export interface Automations {
  automations: Automation[]
}

export interface TokenSpendSettings {
  superintendent_auto_requests_project_summaries: boolean
  project_orchestrators_auto_request_worker_summaries: boolean
  scheduled_automatic_summaries: boolean
  version: string
  updated_by: string
  updated_at_unix_ms: number
}

export type OrchestratorWorkflowProfileSource = 'factory' | 'user' | 'reset'

export interface OrchestratorWorkflowCommand {
  id: string
  capability: string
}

export interface OrchestratorWorkflowAdapterContextFile {
  adapter_id: string
  path: string
}

export interface OrchestratorWorkflowProfile {
  id: string
  name: string
  description: string
  version: string
  instructions_markdown: string
  monitor_interval_ms: string
  commands: OrchestratorWorkflowCommand[]
  adapter_context_files: OrchestratorWorkflowAdapterContextFile[]
  source: OrchestratorWorkflowProfileSource
  updated_by: string
  created_at_unix_ms: number
}

export interface UpdateOrchestratorWorkflowProfileInput {
  actor: string
  expected_version: string
  instructions_markdown: string
  monitor_interval_ms: string
  commands?: OrchestratorWorkflowCommand[]
  adapter_context_files?: OrchestratorWorkflowAdapterContextFile[]
}

export interface ResetOrchestratorWorkflowProfileInput {
  actor: string
  expected_version: string
}

export interface UpdateTokenSpendSettingsInput {
  actor: string
  expected_version: string
  superintendent_auto_requests_project_summaries: boolean
  project_orchestrators_auto_request_worker_summaries: boolean
  scheduled_automatic_summaries: boolean
}

export interface AutomationRuns {
  runs: AutomationRun[]
}

export interface CreateAutomationInput {
  command_id: string
  actor: string
  name: string
  scope: AutomationScope
  placement: CanvasPlacement
  schedule: AutomationSchedule
  selected_project_ids: string[]
  prompt_template: string
}

export interface UpdateAutomationInput {
  command_id: string
  actor: string
  expected_version: string
  name: string
  scope: AutomationScope
  schedule: AutomationSchedule
  selected_project_ids: string[]
  prompt_template: string
}

export interface UpdateAutomationPlacementInput {
  command_id: string
  actor: string
  expected_version: string
  placement: CanvasPlacement
}

export interface UpdateAutomationStateInput {
  command_id: string
  actor: string
  expected_version: string
  paused: boolean
}

export interface RunAutomationInput {
  command_id: string
  actor: string
  expected_version: string
}

export interface PromptAcknowledgement {
  command_id: string
  assignment_id: string
  attempt_id: string
  runtime_status: string
  submitted_at_unix_ms: number
}

export interface OrchestratorPromptAcknowledgement {
  command_id: string
  project_id: string
  worker_id: string
  runtime_status: string
  submitted_at_unix_ms: number
}

export interface YardOrchestratorPromptAcknowledgement {
  command_id: string
  worker_id: string
  runtime_status: string
  submitted_at_unix_ms: number
}

export interface TerminalOutput {
  assignment_id: string
  attempt_id: string
  pane_id: string
  source: string
  format: string
  text: string
  revision: string
  truncated: boolean
}

export interface OrchestratorTerminalOutput {
  project_id: string
  worker_id: string
  pane_id: string
  source: string
  format: string
  text: string
  revision: string
  truncated: boolean
  status_report?: unknown
}

export interface YardOrchestratorTerminalOutput {
  worker_id: string
  pane_id: string
  source: string
  format: string
  text: string
  revision: string
  truncated: boolean
  status_report?: unknown
}

export interface ExternalTerminalLaunch {
  application: 'ghostty'
  terminal_id: string
  command: string[]
}

export interface TerminalFrameMessage {
  type: 'terminal.frame'
  bytes: string
  seq: number
  width: number
  height: number
  full: boolean
}

export interface TerminalClosedMessage {
  type: 'terminal.closed'
  code?: string
  reason?: string
}

export type TerminalServerMessage =
  | TerminalFrameMessage
  | TerminalClosedMessage

export interface TerminalInputMessage {
  type: 'terminal.input'
  text: string
}

export interface TerminalResizeMessage {
  type: 'terminal.resize'
  cols: number
  rows: number
}

export interface TerminalReleaseMessage {
  type: 'terminal.release'
}

export type TerminalClientMessage =
  | TerminalInputMessage
  | TerminalResizeMessage
  | TerminalReleaseMessage

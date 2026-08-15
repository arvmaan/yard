import {
  ArrowRight,
  CircleAlert,
  CircleCheck,
  LoaderCircle,
  Pause,
  Radio,
  WifiOff,
} from 'lucide-react'
import {
  projectUpdates,
  type ProjectStatusReports,
  type ProjectUpdateState,
} from './projectUpdates'
import type {
  Assignment,
  Project,
  RuntimeInventory,
  WorkflowStatus,
  YardOrchestratorRoute,
} from './types'

const UPDATE_STATE = {
  in_progress: {
    icon: LoaderCircle,
    label: 'In progress',
  },
  needs_action: {
    icon: CircleAlert,
    label: 'Needs you',
  },
  offline: {
    icon: WifiOff,
    label: 'Offline',
  },
  ready: {
    icon: Pause,
    label: 'Idle',
  },
} satisfies Record<
  ProjectUpdateState,
  { icon: typeof CircleAlert; label: string }
>

const REPORT_STATE_LABELS: Record<WorkflowStatus, string> = {
  idle: 'Idle',
  needs_attention: 'Needs attention',
  working: 'Working',
}

export function YardOrchestratorOverview({
  assignments,
  inventory,
  onOpenProject,
  projects,
  routes,
  statusReports,
}: {
  assignments: Assignment[]
  inventory: RuntimeInventory | null
  onOpenProject: (project: Project) => void
  projects: Project[]
  routes: YardOrchestratorRoute[]
  statusReports: ProjectStatusReports
}) {
  const updates = projectUpdates(
    projects,
    assignments,
    inventory,
    routes,
    statusReports,
  )
  const attentionCount = updates.filter(
    (update) => update.state === 'needs_action',
  ).length
  const activeCount = updates.filter(
    (update) => update.state === 'in_progress',
  ).length

  return (
    <section
      aria-label="Latest project updates"
      aria-live="polite"
      className="yard-portfolio-updates"
    >
      <div className="yard-portfolio-updates__heading">
        <div>
          <p className="eyebrow">Latest update</p>
        </div>
        <div aria-label="Portfolio status" className="yard-pulse-counts">
          <span data-state="in_progress">
            <Radio aria-hidden="true" size={12} />
            {activeCount} active
          </span>
          <span data-state={attentionCount > 0 ? 'needs_action' : 'ready'}>
            {attentionCount > 0 ? (
              <CircleAlert aria-hidden="true" size={12} />
            ) : (
              <Pause aria-hidden="true" size={12} />
            )}
            {attentionCount} need you
          </span>
        </div>
      </div>

      <div className="yard-project-update-list">
        {updates.map((update) => {
          const state = UPDATE_STATE[update.state]
          const StateIcon = state.icon
          return (
            <button
              className="yard-project-update"
              data-project-id={update.project.id}
              data-source={update.report ? 'status_report' : 'durable'}
              data-state={update.state}
              data-workflow-state={update.report?.state}
              key={update.project.id}
              onClick={() => onOpenProject(update.project)}
              type="button"
            >
              <span className="yard-project-update__project">
                <strong>{update.project.name}</strong>
                <span data-state={update.state}>
                  <StateIcon
                    aria-hidden="true"
                    className={
                      update.state === 'in_progress' ? 'status-spin' : ''
                    }
                    size={12}
                  />
                  {update.report &&
                  update.state ===
                    (update.report.state === 'working'
                      ? 'in_progress'
                      : update.report.state === 'needs_attention'
                        ? 'needs_action'
                        : 'ready')
                    ? REPORT_STATE_LABELS[update.report.state]
                    : state.label}
                </span>
              </span>
              <span className="yard-project-update__summary">
                <span className="yard-project-update__field">
                  <b>Last</b>
                  <span data-field="last">{update.last}</span>
                </span>
                <i aria-hidden="true">/</i>
                <span className="yard-project-update__field">
                  <b>Next</b>
                  <span data-field="next">{update.next}</span>
                </span>
                {update.blockers.length > 0 ? (
                  <span className="yard-project-update__blockers">
                    <b>Blockers</b>
                    <span>
                      {update.blockers.map((blocker, index) => (
                        <span data-field="blocker" key={`${index}:${blocker}`}>
                          {blocker}
                        </span>
                      ))}
                    </span>
                  </span>
                ) : null}
              </span>
              <ArrowRight
                aria-hidden="true"
                className="yard-project-update__arrow"
                size={16}
              />
            </button>
          )
        })}
        {updates.length === 0 ? (
          <div className="yard-project-update-list__empty">
            <CircleCheck aria-hidden="true" size={16} />
            <span>
              <strong>No projects yet</strong>
              <small>Create a project to begin portfolio coordination.</small>
            </span>
          </div>
        ) : null}
      </div>
    </section>
  )
}

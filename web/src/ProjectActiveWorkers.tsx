import type { ProjectDispositionPreview } from './types'
import { workerLabelWithDefault } from './workerDisplay'

// The active workers an archive or delete ends, listed in the one
// confirmation so nothing is cancelled that the user did not see. Yard's own
// summary workers are listed separately: they end with the project too.
export function ProjectActiveWorkers({
  preview,
}: {
  preview: ProjectDispositionPreview
}) {
  const active = preview.active_assignments
  const summaries = preview.summary_worker_assignments ?? []
  if (active.length === 0 && summaries.length === 0) return null
  const ended = active.length + summaries.length
  return (
    <div className="disposition-active-workers">
      {active.length > 0 ? (
        <>
          <p>
            {active.length === 1
              ? 'It also ends this active worker:'
              : `It also ends these ${active.length} active workers:`}
          </p>
          <ul
            aria-label="Active workers to end"
            className="disposition-impact-list"
          >
            {active.map((assignment) => (
              <li key={assignment.assignment_id}>
                <strong>
                  {workerLabelWithDefault({
                    displayName: assignment.worker_display_name,
                    profileName: assignment.profile_name,
                    workerId: assignment.worker_id,
                  })}
                </strong>
                {` — ${assignment.objective}`}
                {assignment.lifecycle === 'handing_off'
                  ? ' (unresolved handoff)'
                  : assignment.lifecycle === 'allocating'
                    ? ' (still starting)'
                    : null}
              </li>
            ))}
          </ul>
        </>
      ) : null}
      {summaries.length > 0 ? (
        <>
          <p>
            {summaries.length === 1
              ? 'It also ends this running summary worker; its summary is not delivered:'
              : `It also ends these ${summaries.length} running summary workers; their summaries are not delivered:`}
          </p>
          <ul
            aria-label="Summary workers to end"
            className="disposition-impact-list"
          >
            {summaries.map((assignment) => (
              <li key={assignment.assignment_id}>
                <strong>{assignment.profile_name}</strong>
                {` — ${assignment.objective}`}
              </li>
            ))}
          </ul>
        </>
      ) : null}
      <p>
        {ended === 1 ? 'Its assignment is' : 'Their assignments are'} recorded
        as cancelled, not completed. The agent
        {ended === 1 ? ' keeps' : 's keep'} running until you close
        {ended === 1 ? ' its Herdr tab' : ' their Herdr tabs'}.
      </p>
    </div>
  )
}

import { CircleAlert, LoaderCircle, RotateCcw } from 'lucide-react'
import type { ArchivedProjectSummary } from './types'

// The Restore state of one archive, keyed by its archive command ID so a
// retry reuses the same restore command.
export interface ProjectRestoreState {
  busy: boolean
  error: string | null
  // Whether sending the same restore command again can succeed (only while
  // Herdr is unreachable or the request got no answer).
  retryable: boolean
}

interface ArchivedProjectsPanelProps {
  archived: ArchivedProjectSummary[] | null
  error: string | null
  onRestore: (summary: ArchivedProjectSummary) => void
  restores: Record<string, ProjectRestoreState>
}

function archivedDetail(summary: ArchivedProjectSummary) {
  const parts = [
    `Archived ${new Date(summary.archived_at_unix_ms).toLocaleString()}`,
  ]
  const cancelled = summary.cancelled_assignment_ids.length
  if (cancelled > 0) {
    parts.push(
      `${cancelled} cancelled ${cancelled === 1 ? 'assignment stays' : 'assignments stay'} cancelled`,
    )
  }
  if (summary.cleanup_pending) parts.push('Herdr tab cleanup pending')
  return parts.join(' · ')
}

// A minimal Archived view: each archived (not deleted) project with Restore
// while its archive is restorable.
export function ArchivedProjectsPanel({
  archived,
  error,
  onRestore,
  restores,
}: ArchivedProjectsPanelProps) {
  return (
    <div className="rail-section rail-section--resources" role="tabpanel">
      <div className="section-heading">
        <div>
          <p className="eyebrow">Projects</p>
          <h2>Archived</h2>
        </div>
        <span>{archived?.length ?? 0}</span>
      </div>
      {error ? (
        <p className="archived-projects__error" role="alert">
          <CircleAlert aria-hidden="true" size={14} />
          {error}
        </p>
      ) : null}
      <ul aria-label="Archived projects" className="resource-list archived-projects">
        {(archived ?? []).map((summary) => {
          const restore = restores[summary.archive_command_id]
          return (
            <li
              className="archived-project-row"
              data-project-id={summary.project_id}
              key={summary.archive_command_id}
            >
              <span>
                <strong>{summary.name}</strong>
                <small>{archivedDetail(summary)}</small>
                {restore?.error ? (
                  <small className="archived-project-row__error" role="alert">
                    {restore.error}
                  </small>
                ) : null}
                {!summary.restorable ? (
                  <small>
                    Not restorable now: its workspace or orchestrator is in
                    use elsewhere.
                  </small>
                ) : null}
              </span>
              <button
                aria-label={`Restore ${summary.name}`}
                className="secondary-button"
                disabled={
                  !summary.restorable ||
                  restore?.busy ||
                  (Boolean(restore?.error) && !restore?.retryable)
                }
                onClick={() => onRestore(summary)}
                type="button"
              >
                {restore?.busy ? (
                  <LoaderCircle
                    aria-hidden="true"
                    className="status-spin"
                    size={14}
                  />
                ) : (
                  <RotateCcw aria-hidden="true" size={14} />
                )}
                <span>
                  {restore?.error && restore.retryable ? 'Retry' : 'Restore'}
                </span>
              </button>
            </li>
          )
        })}
      </ul>
      {archived && archived.length === 0 ? (
        <p className="empty-state">No archived projects.</p>
      ) : null}
      {archived === null && !error ? (
        <p className="empty-state">Loading archived projects…</p>
      ) : null}
    </div>
  )
}

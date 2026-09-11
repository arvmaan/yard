import { useRef } from 'react'
import { CircleAlert, LoaderCircle, Trash2, X } from 'lucide-react'
import type { Project } from './types'
import { useModalDialog } from './useModalDialog'

interface DeleteProjectDialogProps {
  activeAssignmentCount: number
  busy: boolean
  error: string | null
  onClose: () => void
  onConfirm: () => Promise<void>
  project: Project
  returnFocus: HTMLElement | null
}

export function DeleteProjectDialog({
  activeAssignmentCount,
  busy,
  error,
  onClose,
  onConfirm,
  project,
  returnFocus,
}: DeleteProjectDialogProps) {
  const dialogRef = useRef<HTMLElement>(null)
  const blocked = activeAssignmentCount > 0
  useModalDialog({
    canClose: !busy,
    dialogRef,
    onClose,
    returnFocus,
  })

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        aria-labelledby="delete-project-title"
        aria-modal="true"
        className="control-dialog end-session-dialog"
        ref={dialogRef}
        role="dialog"
      >
        <header className="dialog-heading">
          <div>
            <p className="eyebrow">Project disposition</p>
            <h2 id="delete-project-title">Delete project</h2>
          </div>
          <button
            aria-label="Close delete project"
            className="icon-button"
            disabled={busy}
            onClick={onClose}
            title="Close"
            type="button"
          >
            <X aria-hidden="true" size={17} />
          </button>
        </header>
        <div className="end-session-context">
          <Trash2 aria-hidden="true" size={19} />
          <span>
            <strong>{project.name}</strong>
            <small>
              {project.runtime.session} / {project.runtime.workspace_id}
            </small>
          </span>
        </div>
        <div className="end-session-impact">
          <CircleAlert aria-hidden="true" size={17} />
          <p>
            {blocked
              ? `Complete or hand off ${activeAssignmentCount} active assignment${activeAssignmentCount === 1 ? '' : 's'} before deleting.`
              : 'This first archives the project, then permanently removes the project and its orchestrator from Yard views. Durable audit and cleanup records remain, but this UI deletion cannot be undone.'}
          </p>
        </div>
        {error ? (
          <p className="dialog-error" role="alert">
            <CircleAlert aria-hidden="true" size={16} />
            <span>{error}</span>
          </p>
        ) : null}
        <footer className="end-session-actions">
          <button
            className="secondary-button"
            disabled={busy}
            onClick={onClose}
            type="button"
          >
            Keep project
          </button>
          <button
            autoFocus={!blocked}
            className="destructive-button"
            disabled={busy || blocked}
            onClick={() => void onConfirm()}
            type="button"
          >
            {busy ? (
              <LoaderCircle
                aria-hidden="true"
                className="status-spin"
                size={16}
              />
            ) : (
              <Trash2 aria-hidden="true" size={16} />
            )}
            Delete project
          </button>
        </footer>
      </section>
    </div>
  )
}

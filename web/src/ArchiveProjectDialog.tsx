import { useRef } from 'react'
import {
  CircleAlert,
  FolderArchive,
  LoaderCircle,
  X,
} from 'lucide-react'
import type { Project } from './types'
import { useModalDialog } from './useModalDialog'

interface ArchiveProjectDialogProps {
  activeAssignmentCount: number
  busy: boolean
  error: string | null
  onClose: () => void
  onConfirm: () => Promise<void>
  project: Project
  returnFocus: HTMLElement | null
}

export function ArchiveProjectDialog({
  activeAssignmentCount,
  busy,
  error,
  onClose,
  onConfirm,
  project,
  returnFocus,
}: ArchiveProjectDialogProps) {
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
        aria-labelledby="archive-project-title"
        aria-modal="true"
        className="control-dialog end-session-dialog"
        ref={dialogRef}
        role="dialog"
      >
        <header className="dialog-heading">
          <div>
            <p className="eyebrow">Project disposition</p>
            <h2 id="archive-project-title">Archive project</h2>
          </div>
          <button
            aria-label="Close archive project"
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
          <FolderArchive aria-hidden="true" size={19} />
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
              ? `Complete or hand off ${activeAssignmentCount} active assignment${activeAssignmentCount === 1 ? '' : 's'} before archiving.`
              : 'Yard will remove this project from the active map, release its workspace binding, end its orchestrator, and retain its durable history. The Herdr workspace itself is not deleted.'}
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
              <FolderArchive aria-hidden="true" size={16} />
            )}
            Archive project
          </button>
        </footer>
      </section>
    </div>
  )
}

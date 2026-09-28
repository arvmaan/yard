import { useRef } from 'react'
import { CircleAlert, LoaderCircle, Trash2, X } from 'lucide-react'
import {
  projectDispositionBlocker,
  projectDispositionConfirmLabel,
} from './backgroundStatus'
import { ProjectActiveWorkers } from './ProjectActiveWorkers'
import type { Project, ProjectDispositionPreview } from './types'
import { useModalDialog } from './useModalDialog'

interface DeleteProjectDialogProps {
  alreadyArchived: boolean
  busy: boolean
  error: string | null
  onCheckAgain: () => void
  onClose: () => void
  onConfirm: () => Promise<void>
  preview: ProjectDispositionPreview | null
  previewError: string | null
  project: Project
  returnFocus: HTMLElement | null
}

export function DeleteProjectDialog({
  alreadyArchived,
  busy,
  error,
  onCheckAgain,
  onClose,
  onConfirm,
  preview,
  previewError,
  project,
  returnFocus,
}: DeleteProjectDialogProps) {
  const dialogRef = useRef<HTMLElement>(null)
  // Nothing is sent until the preview has listed what the command ends. An
  // already archived project has no active workers left to list.
  const blocker = projectDispositionBlocker(preview)
  const ready = alreadyArchived || (preview !== null && blocker === null)
  const confirmLabel = projectDispositionConfirmLabel('delete', preview)
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
          <div>
            <p>
              {'Yard archives the project if it is still active and removes it and its orchestrator from Yard views in one step. Durable audit and cleanup records remain, and runtime cleanup continues in the background. This cannot be undone.'}
            </p>
            {preview ? <ProjectActiveWorkers preview={preview} /> : null}
            {alreadyArchived ? (
              <p className="disposition-impact-loading" role="status">
                This project was already archived elsewhere; Delete removes
                it permanently.
              </p>
            ) : null}
            {!preview && !previewError ? (
              <p className="disposition-impact-loading">
                Checking what this affects…
              </p>
            ) : null}
          </div>
        </div>
        {blocker && !error ? (
          <p className="dialog-error" role="alert">
            <CircleAlert aria-hidden="true" size={16} />
            <span>{blocker}</span>
          </p>
        ) : null}
        {error ? (
          <p className="dialog-error" role="alert">
            <CircleAlert aria-hidden="true" size={16} />
            <span>{error}</span>
          </p>
        ) : null}
        {previewError && !alreadyArchived ? (
          <p className="dialog-error" role="alert">
            <CircleAlert aria-hidden="true" size={16} />
            <span>{previewError}</span>
            {alreadyArchived ? null : (
              <button
                className="secondary-button"
                disabled={busy}
                onClick={onCheckAgain}
                type="button"
              >
                Check again
              </button>
            )}
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
            autoFocus={ready}
            className="destructive-button"
            disabled={busy || !ready}
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
            {confirmLabel}
          </button>
        </footer>
      </section>
    </div>
  )
}

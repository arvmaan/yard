import { useRef } from 'react'
import {
  CircleAlert,
  FolderArchive,
  LoaderCircle,
  X,
} from 'lucide-react'
import {
  projectDispositionBlocker,
  projectDispositionConfirmLabel,
} from './backgroundStatus'
import { ProjectActiveWorkers } from './ProjectActiveWorkers'
import type { Project, ProjectDispositionPreview } from './types'
import { useModalDialog } from './useModalDialog'

interface ArchiveProjectDialogProps {
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

export function ArchiveProjectDialog({
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
}: ArchiveProjectDialogProps) {
  const dialogRef = useRef<HTMLElement>(null)
  // Nothing is sent until the preview has listed what the command ends.
  const blocker = projectDispositionBlocker(preview)
  const ready = preview !== null && blocker === null
  const confirmLabel = projectDispositionConfirmLabel('archive', preview)
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
          <div>
            <p>
              {'Yard will remove this project from the active map, release its workspace binding, end its orchestrator, and retain its durable history. The Herdr workspace itself is not deleted, and runtime cleanup and knowledge snapshots continue in the background.'}
            </p>
            {preview ? <ProjectActiveWorkers preview={preview} /> : null}
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
        {previewError && !false ? (
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
              <FolderArchive aria-hidden="true" size={16} />
            )}
            {confirmLabel}
          </button>
        </footer>
      </section>
    </div>
  )
}

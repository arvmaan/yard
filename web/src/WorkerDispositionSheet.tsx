import { useRef } from 'react'
import {
  CircleAlert,
  CircleCheck,
  CircleStop,
  LoaderCircle,
  Trash2,
  X,
} from 'lucide-react'
import type { Assignment } from './types'
import { useModalDialog } from './useModalDialog'

export type WorkerDispositionMode = 'end' | 'delete'
export type WorkerDispositionChoice = 'complete' | 'cancel'

interface WorkerDispositionSheetProps {
  assignment: Assignment
  busy: WorkerDispositionChoice | null
  error: string | null
  mode: WorkerDispositionMode
  onClose: () => void
  onConfirm: (choice: WorkerDispositionChoice) => Promise<void>
  returnFocus?: HTMLElement | null
}

/**
 * One compact sheet for ending or deleting a worker that still has an
 * active assignment. The user states how the work ended: completed (a
 * minimal receipt) or ended without completion (a cancellation, never a
 * receipt). Either way Yard ends its session and queues cleanup; it never
 * closes the Herdr tab, so the agent keeps running until the tab is closed.
 */
export function WorkerDispositionSheet({
  assignment,
  busy,
  error,
  mode,
  onClose,
  onConfirm,
  returnFocus,
}: WorkerDispositionSheetProps) {
  const dialogRef = useRef<HTMLElement>(null)
  const initialFocusRef = useRef<HTMLButtonElement>(null)
  const requestClose = useModalDialog({
    canClose: busy === null,
    dialogRef,
    initialFocusRef,
    onClose,
    returnFocus,
  })
  const deleting = mode === 'delete'
  const title = deleting ? 'Delete worker' : 'End session'
  const titleId = `worker-disposition-${mode}-title`
  const completeLabel = deleting ? 'Complete and delete' : 'Complete and end session'
  const disabled = busy !== null

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        aria-labelledby={titleId}
        aria-modal="true"
        className="control-dialog end-session-dialog worker-disposition-sheet"
        ref={dialogRef}
        role="dialog"
      >
        <header className="dialog-heading">
          <div>
            <p className="eyebrow">Active assignment</p>
            <h2 id={titleId}>{title}</h2>
          </div>
          <button
            aria-label={`Close ${title.toLowerCase()}`}
            className="icon-button"
            disabled={disabled}
            onClick={requestClose}
            title="Close"
            type="button"
          >
            <X aria-hidden="true" size={17} />
          </button>
        </header>
        <div className="end-session-context">
          {deleting ? (
            <Trash2 aria-hidden="true" size={19} />
          ) : (
            <CircleStop aria-hidden="true" size={19} />
          )}
          <span>
            <strong>{assignment.profile_name}</strong>
            <small>{assignment.objective}</small>
          </span>
        </div>
        <div className="end-session-impact">
          <CircleAlert aria-hidden="true" size={17} />
          <p>
            This worker is still assigned. Say how the work ended.{' '}
            {deleting
              ? 'Yard then ends the session and removes the worker from its views; audit history stays.'
              : 'Yard then ends the session and queues cleanup.'}{' '}
            The agent keeps running until its Herdr tab is closed.
          </p>
        </div>
        {error ? (
          <p className="dialog-error" role="alert">
            <CircleAlert aria-hidden="true" size={16} />
            <span>{error}</span>
          </p>
        ) : null}
        <footer className="worker-disposition-actions">
          <button
            className="command-button"
            disabled={disabled}
            onClick={() => void onConfirm('complete')}
            ref={initialFocusRef}
            type="button"
          >
            {busy === 'complete' ? (
              <LoaderCircle
                aria-hidden="true"
                className="status-spin"
                size={16}
              />
            ) : (
              <CircleCheck aria-hidden="true" size={16} />
            )}
            {completeLabel}
          </button>
          <button
            aria-describedby={`${titleId}-cancel-note`}
            className="destructive-button"
            disabled={disabled}
            onClick={() => void onConfirm('cancel')}
            type="button"
          >
            {busy === 'cancel' ? (
              <LoaderCircle
                aria-hidden="true"
                className="status-spin"
                size={16}
              />
            ) : (
              <CircleStop aria-hidden="true" size={16} />
            )}
            End without completion
          </button>
          <small
            className="worker-disposition-note"
            id={`${titleId}-cancel-note`}
          >
            Records the work as cancelled, not completed.
          </small>
          <button
            className="secondary-button"
            disabled={disabled}
            onClick={requestClose}
            type="button"
          >
            Cancel
          </button>
        </footer>
      </section>
    </div>
  )
}

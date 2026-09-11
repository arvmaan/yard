import { useRef } from 'react'
import { CircleAlert, LoaderCircle, Trash2, X } from 'lucide-react'
import type { WorkerCandidate } from './types'
import { useModalDialog } from './useModalDialog'

interface DeleteWorkerDialogProps {
  busy: boolean
  candidate: WorkerCandidate
  error: string | null
  onClose: () => void
  onConfirm: () => Promise<void>
}

function candidateLabel(candidate: WorkerCandidate) {
  return (
    candidate.profile_name ??
    `Worker ${candidate.worker.id.slice(0, 8)}`
  )
}

export function DeleteWorkerDialog({
  busy,
  candidate,
  error,
  onClose,
  onConfirm,
}: DeleteWorkerDialogProps) {
  const dialogRef = useRef<HTMLElement>(null)
  const ended = candidate.worker.desired_state === 'ended'
  useModalDialog({
    canClose: !busy,
    dialogRef,
    onClose,
  })

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        aria-labelledby="delete-worker-title"
        aria-modal="true"
        className="control-dialog end-session-dialog"
        ref={dialogRef}
        role="dialog"
      >
        <header className="dialog-heading">
          <div>
            <p className="eyebrow">Worker disposition</p>
            <h2 id="delete-worker-title">Delete worker</h2>
          </div>
          <button
            aria-label="Close delete worker"
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
            <strong>{candidateLabel(candidate)}</strong>
            <small>{candidate.worker.id}</small>
          </span>
        </div>
        <div className="end-session-impact">
          <CircleAlert aria-hidden="true" size={17} />
          <p>
            {ended
              ? 'This permanently removes the ended worker from Yard history. Durable audit and cleanup records remain, but this UI deletion cannot be undone.'
              : 'This first ends the worker session, then permanently removes it from Yard views. Durable audit and cleanup records remain, but this UI deletion cannot be undone.'}
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
            Keep worker
          </button>
          <button
            autoFocus
            className="destructive-button"
            disabled={busy}
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
            Delete worker
          </button>
        </footer>
      </section>
    </div>
  )
}

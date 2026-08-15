import { useRef } from 'react'
import { CircleAlert, CircleStop, LoaderCircle, X } from 'lucide-react'
import type { WorkerCandidate } from './types'
import { useModalDialog } from './useModalDialog'

interface EndWorkerSessionDialogProps {
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

export function EndWorkerSessionDialog({
  busy,
  candidate,
  error,
  onClose,
  onConfirm,
}: EndWorkerSessionDialogProps) {
  const dialogRef = useRef<HTMLElement>(null)
  const runtime = candidate.worker.runtime
  useModalDialog({
    canClose: !busy,
    dialogRef,
    onClose,
  })

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        aria-labelledby="end-worker-session-title"
        aria-modal="true"
        className="control-dialog end-session-dialog"
        ref={dialogRef}
        role="dialog"
      >
        <header className="dialog-heading">
          <div>
            <p className="eyebrow">Worker disposition</p>
            <h2 id="end-worker-session-title">End session</h2>
          </div>
          <button
            aria-label="Close end session"
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
          <CircleStop aria-hidden="true" size={19} />
          <span>
            <strong>{candidateLabel(candidate)}</strong>
            <small>
              {runtime
                ? `${runtime.session} / ${runtime.workspace_id}`
                : 'No live runtime binding'}
            </small>
          </span>
        </div>
        <div className="end-session-impact">
          <CircleAlert aria-hidden="true" size={17} />
          <p>
            {runtime
              ? 'Yard will retain this worker and its completed work for inspection, but its Herdr tab or pane will be closed and it cannot be resumed.'
              : 'Yard will retain this worker and its completed work for inspection, then mark the runtime-less session ended so it cannot be resumed.'}
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
            Keep session
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
              <CircleStop aria-hidden="true" size={16} />
            )}
            End session
          </button>
        </footer>
      </section>
    </div>
  )
}

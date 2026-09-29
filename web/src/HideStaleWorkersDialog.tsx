import { useRef } from 'react'
import { CircleAlert, LoaderCircle, Trash2, X } from 'lucide-react'
import { useModalDialog } from './useModalDialog'

export interface StaleWorkerConfirmationItem {
  id: string
  label: string
}

export function HideStaleWorkersDialog({
  busy,
  error,
  items,
  onClose,
  onConfirm,
  returnFocus,
}: {
  busy: boolean
  error: string | null
  items: StaleWorkerConfirmationItem[]
  onClose: () => void
  onConfirm: () => Promise<void>
  returnFocus: HTMLElement | null
}) {
  const dialogRef = useRef<HTMLElement>(null)
  const confirmRef = useRef<HTMLButtonElement>(null)
  useModalDialog({
    canClose: !busy,
    dialogRef,
    initialFocusRef: confirmRef,
    onClose,
    returnFocus,
  })

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        aria-labelledby="hide-stale-workers-title"
        aria-modal="true"
        className="control-dialog end-session-dialog"
        ref={dialogRef}
        role="dialog"
      >
        <header className="dialog-heading">
          <div>
            <p className="eyebrow">Visibility cleanup</p>
            <h2 id="hide-stale-workers-title">
              Hide {items.length} stale {items.length === 1 ? 'worker' : 'workers'}
            </h2>
          </div>
          <button
            aria-label="Close hide stale workers"
            className="icon-button"
            disabled={busy}
            onClick={onClose}
            title="Close"
            type="button"
          >
            <X aria-hidden="true" size={17} />
          </button>
        </header>
        <div className="end-session-impact">
          <CircleAlert aria-hidden="true" size={17} />
          <p>
            Each listed external worker will be ended if needed, then hidden
            from Yard views. Assignments, artifacts, receipts, and audit history
            remain durable.
          </p>
        </div>
        <ul className="stale-worker-confirmation-list">
          {items.map((item) => (
            <li key={item.id}>
              <strong>{item.label}</strong>
              <code>{item.id}</code>
            </li>
          ))}
        </ul>
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
            Keep workers
          </button>
          <button
            className="destructive-button"
            disabled={busy || items.length === 0}
            onClick={() => void onConfirm()}
            ref={confirmRef}
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
            Hide stale
          </button>
        </footer>
      </section>
    </div>
  )
}

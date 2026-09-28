import { useRef } from 'react'
import {
  CircleAlert,
  FolderArchive,
  LoaderCircle,
  Network,
  Trash2,
  X,
} from 'lucide-react'
import {
  workstreamDispositionBlocker,
  workstreamDispositionImpact,
} from './backgroundStatus'
import type {
  CoordinationNode,
  CoordinationNodeDispositionPreview,
} from './types'
import { useModalDialog } from './useModalDialog'

interface WorkstreamDispositionDialogProps {
  busy: boolean
  error: string | null
  mode: 'archive' | 'delete'
  node: CoordinationNode
  onClose: () => void
  onConfirm: () => Promise<void>
  preview: CoordinationNodeDispositionPreview | null
  previewError: string | null
  returnFocus: HTMLElement | null
}

export function WorkstreamDispositionDialog({
  busy,
  error,
  mode,
  node,
  onClose,
  onConfirm,
  preview,
  previewError,
  returnFocus,
}: WorkstreamDispositionDialogProps) {
  const dialogRef = useRef<HTMLElement>(null)
  const blocker = preview ? workstreamDispositionBlocker(preview) : null
  // Nothing is sent until the preview has itemized what the command does.
  const ready = preview !== null && preview.supported && blocker === null
  const title = mode === 'archive' ? 'Archive workstream' : 'Delete workstream'
  const titleId = `${mode}-workstream-title`
  const Icon = mode === 'archive' ? FolderArchive : Trash2
  useModalDialog({
    canClose: !busy,
    dialogRef,
    onClose,
    returnFocus,
  })

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        aria-labelledby={titleId}
        aria-modal="true"
        className="control-dialog end-session-dialog"
        ref={dialogRef}
        role="dialog"
      >
        <header className="dialog-heading">
          <div>
            <p className="eyebrow">Workstream disposition</p>
            <h2 id={titleId}>{title}</h2>
          </div>
          <button
            aria-label={`Close ${title.toLowerCase()}`}
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
          <Network aria-hidden="true" size={19} />
          <span>
            <strong>{node.name}</strong>
            <small>{node.cwd ?? node.id}</small>
          </span>
        </div>
        <div className="end-session-impact">
          <CircleAlert aria-hidden="true" size={17} />
          <div>
            <p>
              {blocker ??
                (mode === 'archive'
                  ? 'Yard removes this workstream from the map and keeps its durable history. Runtime cleanup continues in the background.'
                  : 'Yard archives this workstream if it is still active and removes it from Yard views in one step. Audit and cleanup records remain. This cannot be undone.')}
            </p>
            {preview && !blocker ? (
              <ul className="disposition-impact-list">
                {workstreamDispositionImpact(preview).map((line) => (
                  <li key={line}>{line}</li>
                ))}
              </ul>
            ) : null}
            {!preview && !previewError ? (
              <p className="disposition-impact-loading">
                Checking what this affects…
              </p>
            ) : null}
          </div>
        </div>
        {previewError || error ? (
          <p className="dialog-error" role="alert">
            <CircleAlert aria-hidden="true" size={16} />
            <span>{error ?? previewError}</span>
          </p>
        ) : null}
        <footer className="end-session-actions">
          <button
            className="secondary-button"
            disabled={busy}
            onClick={onClose}
            type="button"
          >
            Keep workstream
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
              <Icon aria-hidden="true" size={16} />
            )}
            {title}
          </button>
        </footer>
      </section>
    </div>
  )
}

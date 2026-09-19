import { useRef } from 'react'
import { CircleAlert, LoaderCircle, X } from 'lucide-react'
import type {
  CompletedRuntimeCleanupPreview,
  CompletedRuntimeRetentionReason,
} from './types'
import { useModalDialog } from './useModalDialog'

interface CompletedRuntimeCleanupPreviewDialogProps {
  error: string | null
  loading: boolean
  onClose: () => void
  preview: CompletedRuntimeCleanupPreview | null
  returnFocus: HTMLElement | null
}

function retentionLabel(reason: CompletedRuntimeRetentionReason) {
  const label = reason.replaceAll('_', ' ')
  return label.charAt(0).toUpperCase() + label.slice(1)
}

export function CompletedRuntimeCleanupPreviewDialog({
  error,
  loading,
  onClose,
  preview,
  returnFocus,
}: CompletedRuntimeCleanupPreviewDialogProps) {
  const dialogRef = useRef<HTMLElement>(null)
  useModalDialog({ dialogRef, onClose, returnFocus })

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        aria-labelledby="completed-runtime-cleanup-preview-title"
        aria-modal="true"
        className="control-dialog end-session-dialog"
        ref={dialogRef}
        role="dialog"
      >
        <header className="dialog-heading">
          <div>
            <p className="eyebrow">Read-only review</p>
            <h2 id="completed-runtime-cleanup-preview-title">
              Cleanup safety preview
            </h2>
          </div>
          <button
            aria-label="Dismiss cleanup preview"
            className="icon-button"
            onClick={onClose}
            title="Dismiss"
            type="button"
          >
            <X aria-hidden="true" size={17} />
          </button>
        </header>
        <div className="end-session-impact">
          <CircleAlert aria-hidden="true" size={17} />
          <p>Nothing will be closed. This preview explains why each runtime is retained.</p>
        </div>
        {loading ? (
          <p>
            <LoaderCircle
              aria-hidden="true"
              className="status-spin"
              size={16}
            />{' '}
            Loading preview…
          </p>
        ) : error ? (
          <p className="dialog-error" role="alert">
            {error}
          </p>
        ) : preview ? (
          <>
            <p>
              {preview.candidate_count} completed Yard-created runtime
              {preview.candidate_count === 1 ? '' : 's'} reviewed; all retained.
            </p>
            {preview.truncated ? (
              <p>
                Showing the first {preview.limit}; additional matching runtimes
                are retained outside this preview.
              </p>
            ) : null}
            {preview.candidates.length === 0 ? (
              <p>No completed Yard-created runtimes were found.</p>
            ) : (
              <ul>
                {preview.candidates.map((candidate) => (
                  <li key={candidate.assignment_id}>
                    <strong>{candidate.profile_name}</strong> —{' '}
                    {candidate.project_name} / {candidate.role}
                    <br />
                    <small>
                      {candidate.linked_artifact_count} linked artifact
                      {candidate.linked_artifact_count === 1 ? '' : 's'};
                      retained:{' '}
                      {candidate.retained_reasons
                        .map(retentionLabel)
                        .join(', ')}
                    </small>
                  </li>
                ))}
              </ul>
            )}
          </>
        ) : null}
      </section>
    </div>
  )
}

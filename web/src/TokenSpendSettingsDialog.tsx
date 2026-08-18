import {
  useId,
  useRef,
  useState,
  type FormEvent,
} from 'react'
import { CircleAlert, LoaderCircle, Save, Settings2, X } from 'lucide-react'
import type { TokenSpendSettings } from './types'
import { useModalDialog } from './useModalDialog'

interface TokenSpendSelection {
  superintendent: boolean
  projectOrchestrators: boolean
  scheduled: boolean
}

export function TokenSpendSettingsDialog({
  busy,
  error,
  onClose,
  onSave,
  returnFocus,
  settings,
}: {
  busy: boolean
  error: string | null
  onClose: () => void
  onSave: (selection: TokenSpendSelection) => void
  returnFocus: HTMLElement | null
  settings: TokenSpendSettings
}) {
  const titleId = useId()
  const dialogRef = useRef<HTMLFormElement>(null)
  const [superintendent, setSuperintendent] = useState(
    settings.superintendent_auto_requests_project_summaries,
  )
  const [projectOrchestrators, setProjectOrchestrators] = useState(
    settings.project_orchestrators_auto_request_worker_summaries,
  )
  const [scheduled, setScheduled] = useState(
    settings.scheduled_automatic_summaries,
  )
  const dirty =
    superintendent !==
      settings.superintendent_auto_requests_project_summaries ||
    projectOrchestrators !==
      settings.project_orchestrators_auto_request_worker_summaries ||
    scheduled !== settings.scheduled_automatic_summaries

  useModalDialog({
    canClose: !busy,
    dialogRef,
    onClose,
    returnFocus,
  })

  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (!dirty || busy) return
    onSave({ projectOrchestrators, scheduled, superintendent })
  }

  return (
    <div className="modal-backdrop token-spend-dialog-backdrop">
      <form
        aria-labelledby={titleId}
        aria-modal="true"
        className="control-dialog token-spend-dialog"
        onSubmit={submit}
        ref={dialogRef}
        role="dialog"
      >
        <header className="token-spend-dialog__header">
          <span className="token-spend-dialog__icon">
            <Settings2 aria-hidden="true" size={19} />
          </span>
          <div>
            <p className="eyebrow">Automatic token use</p>
            <h2 id={titleId}>Coordination settings</h2>
          </div>
          <button
            aria-label="Close coordination settings"
            className="icon-button"
            disabled={busy}
            onClick={onClose}
            title="Close"
            type="button"
          >
            <X aria-hidden="true" size={18} />
          </button>
        </header>

        <fieldset className="token-spend-options">
          <legend>Automatic behavior</legend>
          <label className="token-spend-option">
            <span>
              <strong>Superintendent to project orchestrators</strong>
              <small>Automatically request project summaries.</small>
            </span>
            <span className="token-spend-switch">
              <input
                checked={superintendent}
                onChange={(event) => setSuperintendent(event.target.checked)}
                role="switch"
                type="checkbox"
              />
              <span aria-hidden="true" />
            </span>
          </label>

          <label className="token-spend-option">
            <span>
              <strong>Project orchestrators to workers</strong>
              <small>Automatically request worker summaries.</small>
            </span>
            <span className="token-spend-switch">
              <input
                checked={projectOrchestrators}
                onChange={(event) =>
                  setProjectOrchestrators(event.target.checked)
                }
                role="switch"
                type="checkbox"
              />
              <span aria-hidden="true" />
            </span>
          </label>

          <label className="token-spend-option">
            <span>
              <strong>Scheduled automatic summaries</strong>
              <small>Dispatch enabled automation schedules when due.</small>
            </span>
            <span className="token-spend-switch">
              <input
                checked={scheduled}
                onChange={(event) => setScheduled(event.target.checked)}
                role="switch"
                type="checkbox"
              />
              <span aria-hidden="true" />
            </span>
          </label>
        </fieldset>

        <section className="token-spend-manual">
          <p className="eyebrow">Manual actions</p>
          <p>Prompts, routes, and Run now remain independently available.</p>
        </section>

        {error ? (
          <p className="dialog-error token-spend-dialog__error" role="alert">
            <CircleAlert aria-hidden="true" size={16} />
            <span>{error}</span>
          </p>
        ) : null}

        <footer className="dialog-actions token-spend-dialog__actions">
          <button
            className="secondary-button"
            disabled={busy}
            onClick={onClose}
            type="button"
          >
            Cancel
          </button>
          <button className="command-button" disabled={busy || !dirty}>
            {busy ? (
              <LoaderCircle
                aria-hidden="true"
                className="status-spin"
                size={16}
              />
            ) : (
              <Save aria-hidden="true" size={16} />
            )}
            Save automatic settings
          </button>
        </footer>
      </form>
    </div>
  )
}

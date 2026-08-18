import {
  useId,
  useRef,
  useState,
  type FormEvent,
} from 'react'
import {
  CircleAlert,
  LoaderCircle,
  RotateCcw,
  Save,
  Workflow,
  X,
} from 'lucide-react'
import type { OrchestratorWorkflowProfile } from './types'
import { useModalDialog } from './useModalDialog'

export function OrchestratorWorkflowProfileDialog({
  busy,
  error,
  onClose,
  onReset,
  onSave,
  profile,
  returnFocus,
}: {
  busy: boolean
  error: string | null
  onClose: () => void
  onReset: () => void
  onSave: (input: {
    instructionsMarkdown: string
    monitorIntervalMs: number
  }) => void
  profile: OrchestratorWorkflowProfile
  returnFocus: HTMLElement | null
}) {
  const titleId = useId()
  const dialogRef = useRef<HTMLFormElement>(null)
  const [instructionsMarkdown, setInstructionsMarkdown] = useState(
    profile.instructions_markdown,
  )
  const [monitorMinutes, setMonitorMinutes] = useState(
    String(Number(profile.monitor_interval_ms) / 60_000),
  )
  const monitorIntervalMs = Number(monitorMinutes) * 60_000
  const validInterval =
    Number.isInteger(monitorIntervalMs) &&
    monitorIntervalMs >= 60_000 &&
    monitorIntervalMs <= 86_400_000
  const dirty =
    instructionsMarkdown.trim() !== profile.instructions_markdown.trim() ||
    String(monitorIntervalMs) !== profile.monitor_interval_ms

  useModalDialog({
    canClose: !busy,
    dialogRef,
    onClose,
    returnFocus,
  })

  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (!dirty || !instructionsMarkdown.trim() || !validInterval || busy) {
      return
    }
    onSave({
      instructionsMarkdown: instructionsMarkdown.trim(),
      monitorIntervalMs,
    })
  }

  const sourceLabel =
    profile.source === 'user'
      ? 'Custom'
      : profile.source === 'reset'
        ? 'Factory reset'
        : 'Factory'

  return (
    <div className="modal-backdrop workflow-profile-dialog-backdrop">
      <form
        aria-labelledby={titleId}
        aria-modal="true"
        className="control-dialog workflow-profile-dialog"
        onSubmit={submit}
        ref={dialogRef}
        role="dialog"
      >
        <header className="workflow-profile-dialog__header">
          <span className="workflow-profile-dialog__icon">
            <Workflow aria-hidden="true" size={19} />
          </span>
          <div>
            <p className="eyebrow">Yard orchestrator</p>
            <h2 id={titleId}>Orchestrator workflow</h2>
          </div>
          <button
            aria-label="Close orchestrator workflow"
            className="icon-button"
            disabled={busy}
            onClick={onClose}
            title="Close"
            type="button"
          >
            <X aria-hidden="true" size={18} />
          </button>
        </header>

        <div className="workflow-profile-dialog__body">
          <dl className="workflow-profile-meta">
            <div>
              <dt>Revision</dt>
              <dd>{profile.version}</dd>
            </div>
            <div>
              <dt>Source</dt>
              <dd>{sourceLabel}</dd>
            </div>
            <div>
              <dt>Cadence</dt>
              <dd>{Number(profile.monitor_interval_ms) / 60_000} min</dd>
            </div>
          </dl>

          <label className="workflow-profile-field">
            <span>Monitor interval (minutes)</span>
            <input
              aria-invalid={!validInterval}
              disabled={busy}
              max={1440}
              min={1}
              onChange={(event) => setMonitorMinutes(event.target.value)}
              step={1}
              type="number"
              value={monitorMinutes}
            />
          </label>

          <label className="workflow-profile-field">
            <span>Workflow instructions (Markdown)</span>
            <textarea
              disabled={busy}
              onChange={(event) =>
                setInstructionsMarkdown(event.target.value)
              }
              required
              rows={18}
              spellCheck
              value={instructionsMarkdown}
            />
          </label>
        </div>

        {error ? (
          <p className="dialog-error workflow-profile-dialog__error" role="alert">
            <CircleAlert aria-hidden="true" size={16} />
            <span>{error}</span>
          </p>
        ) : null}

        <footer className="dialog-actions workflow-profile-dialog__actions">
          <button
            className="secondary-button"
            disabled={busy}
            onClick={onReset}
            type="button"
          >
            <RotateCcw aria-hidden="true" size={16} />
            Reset to factory
          </button>
          <span />
          <button
            className="secondary-button"
            disabled={busy}
            onClick={onClose}
            type="button"
          >
            Cancel
          </button>
          <button
            className="command-button"
            disabled={
              busy ||
              !dirty ||
              !instructionsMarkdown.trim() ||
              !validInterval
            }
          >
            {busy ? (
              <LoaderCircle
                aria-hidden="true"
                className="status-spin"
                size={16}
              />
            ) : (
              <Save aria-hidden="true" size={16} />
            )}
            Save changes
          </button>
        </footer>
      </form>
    </div>
  )
}

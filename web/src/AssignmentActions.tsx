import {
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
} from 'react'
import {
  CircleAlert,
  CircleCheck,
  CircleStop,
  Ellipsis,
  FileCheck2,
  LoaderCircle,
  RotateCcw,
  Trash2,
  Undo2,
} from 'lucide-react'
import type { Assignment } from './types'

interface AssignmentActionsProps {
  assignment: Assignment
  busy: boolean
  completeAndEndSession: boolean
  error: string | null
  onComplete: () => void
  onCompleteAndEndSessionChange: (enabled: boolean) => void
  onCompleteWithDetails: () => void
  onDelete?: () => void
  onEndSession?: () => void
  onRetry?: () => void
  onUndo: () => void
  pending: boolean
}

/**
 * Worker actions for an active assignment: one primary `Complete` (sent
 * after a short pre-commit Undo window) and an inline overflow with the
 * detailed form, End session, and Delete worker. The overflow renders
 * inline because the inspector clips positioned popovers.
 */
export function AssignmentActions({
  assignment,
  busy,
  completeAndEndSession,
  error,
  onComplete,
  onCompleteAndEndSessionChange,
  onCompleteWithDetails,
  onDelete,
  onEndSession,
  onRetry,
  onUndo,
  pending,
}: AssignmentActionsProps) {
  const id = useId()
  const [overflowOpen, setOverflowOpen] = useState(false)
  const toggleRef = useRef<HTMLButtonElement>(null)
  const completeRef = useRef<HTMLButtonElement>(null)
  const undoRef = useRef<HTMLButtonElement>(null)
  // Complete and Undo replace each other, so focus would fall to the page
  // body. Move it to the button that took the pressed one's place, so a
  // keyboard user can reach Undo inside its short window.
  const focusUndoNext = useRef(false)
  const undoHasFocus = useRef(false)

  useLayoutEffect(() => {
    if (pending) {
      if (focusUndoNext.current) undoRef.current?.focus()
      focusUndoNext.current = false
      return
    }
    if (undoHasFocus.current) {
      undoHasFocus.current = false
      completeRef.current?.focus()
    }
  }, [pending])
  const descriptionId = `${id}-complete-description`
  const overflowId = `${id}-overflow`

  const closeOverflow = (restoreFocus: boolean) => {
    setOverflowOpen(false)
    if (restoreFocus) toggleRef.current?.focus()
  }
  const onOverflowKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Escape') {
      event.stopPropagation()
      closeOverflow(true)
    }
  }
  // Focus returns to the overflow toggle first, so a dialog opened by the
  // action restores focus there when it closes.
  const choose = (action: () => void) => () => {
    closeOverflow(true)
    action()
  }

  return (
    <div className="inspector-actions assignment-actions">
      {pending ? (
        <div className="quick-complete-pending" role="status">
          <LoaderCircle aria-hidden="true" className="status-spin" size={16} />
          <span>
            <strong>Completing…</strong>
            <small>
              {completeAndEndSession
                ? 'Then Yard ends the session.'
                : 'The session stays open.'}
            </small>
          </span>
          <button
            className="secondary-button"
            onBlur={() => {
              undoHasFocus.current = false
            }}
            onClick={onUndo}
            onFocus={() => {
              undoHasFocus.current = true
            }}
            ref={undoRef}
            type="button"
          >
            <Undo2 aria-hidden="true" size={15} />
            Undo
          </button>
        </div>
      ) : (
        <div className="assignment-primary-actions">
          <button
            aria-describedby={descriptionId}
            className="command-button"
            disabled={busy}
            onClick={() => {
              focusUndoNext.current = true
              onComplete()
            }}
            ref={completeRef}
            type="button"
          >
            {busy ? (
              <LoaderCircle
                aria-hidden="true"
                className="status-spin"
                size={16}
              />
            ) : (
              <CircleCheck aria-hidden="true" size={16} />
            )}
            Complete
          </button>
          <button
            aria-controls={overflowId}
            aria-expanded={overflowOpen}
            aria-label="More worker actions"
            className="icon-button assignment-more-button"
            disabled={busy}
            onClick={() => setOverflowOpen((open) => !open)}
            ref={toggleRef}
            title="More worker actions"
            type="button"
          >
            <Ellipsis aria-hidden="true" size={17} />
          </button>
        </div>
      )}
      <small className="inspector-action-note" id={descriptionId}>
        {completeAndEndSession
          ? `Completes “${assignment.objective}” and ends the session.`
          : `Completes “${assignment.objective}”; the session stays open.`}
      </small>
      {error ? (
        <div className="assignment-action-error" role="alert">
          <CircleAlert aria-hidden="true" size={16} />
          <span>{error}</span>
          {onRetry ? (
            <button
              className="secondary-button"
              disabled={busy}
              onClick={onRetry}
              type="button"
            >
              <RotateCcw aria-hidden="true" size={14} />
              Retry
            </button>
          ) : null}
        </div>
      ) : null}
      {overflowOpen && !pending ? (
        <div
          aria-label="More worker actions"
          className="assignment-overflow"
          id={overflowId}
          onKeyDown={onOverflowKeyDown}
          role="group"
        >
          <button
            className="secondary-button"
            onClick={choose(onCompleteWithDetails)}
            type="button"
          >
            <FileCheck2 aria-hidden="true" size={15} />
            Complete with details…
          </button>
          {onEndSession ? (
            <button
              className="secondary-button"
              onClick={choose(onEndSession)}
              type="button"
            >
              <CircleStop aria-hidden="true" size={15} />
              End session
            </button>
          ) : null}
          {onDelete ? (
            <button
              className="destructive-button"
              onClick={choose(onDelete)}
              type="button"
            >
              <Trash2 aria-hidden="true" size={15} />
              Delete worker
            </button>
          ) : null}
          <label className="assignment-overflow__preference">
            <input
              checked={completeAndEndSession}
              onChange={(event) =>
                onCompleteAndEndSessionChange(event.target.checked)
              }
              type="checkbox"
            />
            <span>
              Complete and end session
              <small>Stored in this browser</small>
            </span>
          </label>
        </div>
      ) : null}
    </div>
  )
}

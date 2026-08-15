import { useEffect, useRef, useState, type FormEvent } from 'react'
import { ArrowRightLeft, CircleAlert, LoaderCircle, X } from 'lucide-react'
import type {
  Assignment,
  HandoffTargetRole,
  Project,
} from './types'
import { useModalDialog } from './useModalDialog'

export interface HandoffDetails {
  objective: string
  role: string
  targetRole: HandoffTargetRole
}

interface HandoffDialogProps {
  busy: boolean
  error: string | null
  sourceAssignment: Assignment
  sourceProject: Project
  targetProject: Project
  onClose: () => void
  onConfirm: (details: HandoffDetails) => Promise<void>
}

export function HandoffDialog({
  busy,
  error,
  sourceAssignment,
  sourceProject,
  targetProject,
  onClose,
  onConfirm,
}: HandoffDialogProps) {
  const dialogRef = useRef<HTMLElement>(null)
  const [objective, setObjective] = useState(sourceAssignment.objective)
  const [role, setRole] = useState(sourceAssignment.role)
  const [targetRole, setTargetRole] = useState<HandoffTargetRole>('member')
  const sourceKey = `${sourceAssignment.id}:${sourceAssignment.version}`
  useModalDialog({
    canClose: !busy,
    dialogRef,
    onClose,
  })

  useEffect(() => {
    setObjective(sourceAssignment.objective)
    setRole(sourceAssignment.role)
    setTargetRole('member')
  }, [sourceAssignment.objective, sourceAssignment.role, sourceKey, targetProject.id])

  const submit = (event: FormEvent) => {
    event.preventDefault()
    void onConfirm({ objective, role, targetRole })
  }

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        aria-labelledby="handoff-title"
        aria-modal="true"
        className="control-dialog handoff-dialog"
        ref={dialogRef}
        role="dialog"
      >
        <header className="dialog-heading">
          <div>
            <p className="eyebrow">Confirm handoff</p>
            <h2 id="handoff-title">Move assigned worker</h2>
          </div>
          <button
            aria-label="Close handoff"
            className="icon-button"
            disabled={busy}
            onClick={onClose}
            title="Close"
            type="button"
          >
            <X aria-hidden="true" size={17} />
          </button>
        </header>
        <div className="allocation-route" aria-label="Handoff route">
          <span>{sourceProject.name}</span>
          <ArrowRightLeft aria-hidden="true" size={15} />
          <strong>{targetProject.name}</strong>
        </div>
        <form className="dialog-form" onSubmit={submit}>
          <label>
            <span>Target role</span>
            <select
              onChange={(event) =>
                setTargetRole(event.target.value as HandoffTargetRole)
              }
              value={targetRole}
            >
              <option value="member">Project worker</option>
              <option value="orchestrator">Project orchestrator</option>
            </select>
          </label>
          {targetRole === 'orchestrator' ? (
            <div className="handoff-impact">
              <CircleAlert aria-hidden="true" size={16} />
              <span>
                Replaces worker {targetProject.orchestrator.id.slice(0, 12)} as
                the project orchestrator.
              </span>
            </div>
          ) : null}
          <label>
            <span>Objective</span>
            <textarea
              autoFocus
              maxLength={16000}
              onChange={(event) => setObjective(event.target.value)}
              required
              rows={5}
              value={objective}
            />
          </label>
          <div className="form-grid">
            <label>
              <span>Role</span>
              <input
                maxLength={240}
                onChange={(event) => setRole(event.target.value)}
                required
                value={role}
              />
            </label>
            <label>
              <span>Isolation</span>
              <select disabled value="project_workspace">
                <option value="project_workspace">Project workspace</option>
              </select>
            </label>
          </div>
          {error ? (
            <div className="dialog-error" role="alert">
              <CircleAlert aria-hidden="true" size={16} />
              <span>{error}</span>
            </div>
          ) : null}
          <footer className="dialog-actions">
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
              disabled={busy || !objective.trim() || !role.trim()}
              type="submit"
            >
              {busy ? (
                <LoaderCircle
                  aria-hidden="true"
                  className="status-spin"
                  size={16}
                />
              ) : (
                <ArrowRightLeft aria-hidden="true" size={16} />
              )}
              Confirm handoff
            </button>
          </footer>
        </form>
      </section>
    </div>
  )
}

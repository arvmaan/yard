import { useEffect, useRef, useState, type FormEvent } from 'react'
import { CircleAlert, LoaderCircle, Send, X } from 'lucide-react'
import type { Project, WorkerCandidate, WorkerProfile } from './types'
import { useModalDialog } from './useModalDialog'

export type AllocationSubject =
  | { kind: 'profile'; profile: WorkerProfile }
  | { kind: 'worker'; candidate: WorkerCandidate }

export interface AllocationDetails {
  objective: string
  role: string
  profileId?: string
}

interface AllocationDialogProps {
  busy: boolean
  error: string | null
  profiles: WorkerProfile[]
  project: Project
  subject: AllocationSubject
  onClose: () => void
  onConfirm: (details: AllocationDetails) => Promise<void>
}

function actionLabel(subject: AllocationSubject) {
  if (subject.kind === 'profile') return 'Create worker'
  return subject.candidate.availability === 'resumable'
    ? 'Resume worker'
    : 'Assign worker'
}

function subjectLabel(subject: AllocationSubject) {
  if (subject.kind === 'profile') return subject.profile.name
  return (
    subject.candidate.profile_name ??
    `Worker ${subject.candidate.worker.id.slice(0, 8)}`
  )
}

export function AllocationDialog({
  busy,
  error,
  profiles,
  project,
  subject,
  onClose,
  onConfirm,
}: AllocationDialogProps) {
  const dialogRef = useRef<HTMLElement>(null)
  const linkedProfile =
    subject.kind === 'worker' && subject.candidate.worker.profile_id
      ? profiles.find(
          (profile) =>
            profile.id === subject.candidate.worker.profile_id &&
            profile.version === subject.candidate.worker.profile_version,
        )
      : undefined
  const needsProfile =
    subject.kind === 'worker' && !subject.candidate.worker.profile_id
  const initialProfileId = ''
  const initialRole =
    subject.kind === 'profile'
      ? subject.profile.default_role
      : (subject.candidate.default_role ??
        linkedProfile?.default_role ??
        'worker')
  const [objective, setObjective] = useState('')
  const [role, setRole] = useState(initialRole)
  const [profileId, setProfileId] = useState(initialProfileId)
  const label = actionLabel(subject)
  const sourceKey =
    subject.kind === 'profile'
      ? `profile:${subject.profile.id}:${subject.profile.version}`
      : `worker:${subject.candidate.worker.id}:${subject.candidate.worker.version}`
  useModalDialog({
    canClose: !busy,
    dialogRef,
    onClose,
  })

  useEffect(() => {
    setObjective('')
    setRole(initialRole)
    setProfileId(initialProfileId)
  }, [initialProfileId, initialRole, project.id, sourceKey])

  const submit = (event: FormEvent) => {
    event.preventDefault()
    void onConfirm({
      objective,
      role,
      ...(needsProfile ? { profileId } : {}),
    })
  }

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        aria-labelledby="allocation-title"
        aria-modal="true"
        className="control-dialog allocation-dialog"
        ref={dialogRef}
        role="dialog"
      >
        <header className="dialog-heading">
          <div>
            <p className="eyebrow">Confirm allocation</p>
            <h2 id="allocation-title">{label}</h2>
          </div>
          <button
            aria-label="Close allocation"
            className="icon-button"
            disabled={busy}
            onClick={onClose}
            title="Close"
            type="button"
          >
            <X aria-hidden="true" size={17} />
          </button>
        </header>
        <div className="allocation-route" aria-label="Allocation route">
          <span>{subjectLabel(subject)}</span>
          <strong>{project.name}</strong>
        </div>
        <form className="dialog-form" onSubmit={submit}>
          {needsProfile ? (
            <label>
              <span>Worker profile</span>
              <select
                onChange={(event) => {
                  const nextProfileId = event.target.value
                  setProfileId(nextProfileId)
                  const nextProfile = profiles.find(
                    (profile) => profile.id === nextProfileId,
                  )
                  if (nextProfile) setRole(nextProfile.default_role)
                }}
                value={profileId}
              >
                <option value="">Blank profile</option>
                {profiles.map((profile) => (
                  <option key={profile.id} value={profile.id}>
                    {profile.name}
                  </option>
                ))}
              </select>
            </label>
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
              disabled={
                busy ||
                !objective.trim() ||
                !role.trim()
              }
              type="submit"
            >
              {busy ? (
                <LoaderCircle
                  aria-hidden="true"
                  className="status-spin"
                  size={16}
                />
              ) : (
                <Send aria-hidden="true" size={16} />
              )}
              {label}
            </button>
          </footer>
        </form>
      </section>
    </div>
  )
}

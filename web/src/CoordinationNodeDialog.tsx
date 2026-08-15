import { useEffect, useId, useRef, useState, type FormEvent } from 'react'
import {
  Database,
  LoaderCircle,
  Network,
  Plus,
  X,
} from 'lucide-react'
import type {
  CanvasPlacement,
  CoordinationNodeKind,
  Project,
  WorkerProfile,
} from './types'
import { useModalDialog } from './useModalDialog'

export interface CoordinationNodeCreationDetails {
  attachedProjectIds: string[]
  kind: CoordinationNodeKind
  name: string
  placement: CanvasPlacement
  profileId: string | null
}

export function CoordinationNodeDialog({
  busy,
  error,
  initialKind,
  onClose,
  onCreate,
  placement,
  profiles,
  projects,
}: {
  busy: boolean
  error: string | null
  initialKind: CoordinationNodeKind
  onClose: () => void
  onCreate: (details: CoordinationNodeCreationDetails) => void
  placement: CanvasPlacement
  profiles: WorkerProfile[]
  projects: Project[]
}) {
  const titleId = useId()
  const dialogRef = useRef<HTMLFormElement>(null)
  const [kind, setKind] = useState<CoordinationNodeKind>(initialKind)
  const [name, setName] = useState('')
  const [profileId, setProfileId] = useState('')
  const [attachedProjectIds, setAttachedProjectIds] = useState<string[]>([])
  const eligibleProfiles = profiles.filter(
    (profile) => profile.runtime_adapter === 'herdr',
  )

  useEffect(() => {
    if (!eligibleProfiles.some((profile) => profile.id === profileId)) {
      setProfileId(
        eligibleProfiles.find(
          (profile) => profile.default_role === 'orchestrator',
        )?.id ??
          eligibleProfiles[0]?.id ??
          '',
      )
    }
  }, [eligibleProfiles, profileId])

  useModalDialog({
    canClose: !busy,
    dialogRef,
    onClose,
  })

  const submit = (event: FormEvent) => {
    event.preventDefault()
    const trimmedName = name.trim()
    if (!trimmedName) return
    onCreate({
      attachedProjectIds,
      kind,
      name: trimmedName,
      placement,
      profileId: kind === 'workstream' && profileId ? profileId : null,
    })
  }

  return (
    <div className="modal-backdrop coordination-dialog-backdrop">
      <form
        aria-labelledby={titleId}
        aria-modal="true"
        className="coordination-dialog"
        onSubmit={submit}
        ref={dialogRef}
        role="dialog"
      >
        <header className="coordination-dialog__header">
          <span className="coordination-dialog__icon">
            <Plus aria-hidden="true" size={19} />
          </span>
          <div>
            <p className="eyebrow">Map infrastructure</p>
            <h2 id={titleId}>Create node</h2>
          </div>
          <button
            aria-label="Close node creation"
            className="icon-button"
            disabled={busy}
            onClick={onClose}
            title="Close"
            type="button"
          >
            <X aria-hidden="true" size={18} />
          </button>
        </header>

        <div className="coordination-kind-picker" role="group" aria-label="Node type">
          <button
            aria-pressed={kind === 'workstream'}
            onClick={() => setKind('workstream')}
            type="button"
          >
            <Network aria-hidden="true" size={20} />
            <span>
              <strong>Workstream</strong>
              <small>Coordinates attached projects</small>
            </span>
          </button>
          <button
            aria-pressed={kind === 'knowledge_store'}
            onClick={() => setKind('knowledge_store')}
            type="button"
          >
            <Database aria-hidden="true" size={20} />
            <span>
              <strong>Knowledge store</strong>
              <small>Collects project snapshots</small>
            </span>
          </button>
        </div>

        <label className="field-label" htmlFor="coordination-node-name">
          Name
        </label>
        <input
          autoFocus
          id="coordination-node-name"
          maxLength={120}
          onChange={(event) => setName(event.target.value)}
          placeholder={
            kind === 'workstream'
              ? 'Release readiness'
              : 'Platform knowledge'
          }
          value={name}
        />

        {kind === 'workstream' ? (
          <>
            <label className="field-label" htmlFor="coordination-node-profile">
              Orchestrator profile
            </label>
            <select
              disabled={eligibleProfiles.length === 0}
              id="coordination-node-profile"
              onChange={(event) => setProfileId(event.target.value)}
              value={profileId}
            >
              {eligibleProfiles.length === 0 ? (
                <option value="">Create without a worker</option>
              ) : null}
              {eligibleProfiles.map((profile) => (
                <option key={profile.id} value={profile.id}>
                  {profile.name} / {profile.provider}
                </option>
              ))}
            </select>
          </>
        ) : null}

        <fieldset className="coordination-project-picker">
          <legend>Attached projects</legend>
          {projects.length === 0 ? (
            <p className="empty-state">No projects are available.</p>
          ) : (
            projects.map((project) => (
              <label key={project.id}>
                <input
                  checked={attachedProjectIds.includes(project.id)}
                  onChange={(event) =>
                    setAttachedProjectIds((current) =>
                      event.target.checked
                        ? [...current, project.id]
                        : current.filter((id) => id !== project.id),
                    )
                  }
                  type="checkbox"
                />
                <span>{project.name}</span>
              </label>
            ))
          )}
        </fieldset>

        {error ? (
          <p className="form-error" role="alert">
            {error}
          </p>
        ) : null}

        <footer className="coordination-dialog__actions">
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
            disabled={busy || !name.trim()}
            type="submit"
          >
            {busy ? (
              <LoaderCircle
                aria-label="Creating node"
                className="status-spin"
                size={16}
              />
            ) : kind === 'workstream' ? (
              <Network aria-hidden="true" size={16} />
            ) : (
              <Database aria-hidden="true" size={16} />
            )}
            Create node
          </button>
        </footer>
      </form>
    </div>
  )
}

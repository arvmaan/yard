import { useEffect, useRef, useState, type FormEvent } from 'react'
import {
  CircleAlert,
  CircleCheck,
  FileCode2,
  LoaderCircle,
  Trash2,
  X,
} from 'lucide-react'
import type { ArtifactKind, Assignment } from './types'
import { useModalDialog } from './useModalDialog'

const MAX_ARTIFACT_BYTES = 1_048_576
const MAX_ARTIFACTS = 64

export interface PendingArtifact {
  id: string
  file: File
  kind: ArtifactKind
}

export interface CompletionDetails {
  summary: string
  artifactRefs: string[]
  artifacts: PendingArtifact[]
  evidenceRefs: string[]
  unresolvedBlockers: string[]
}

interface CompletionDialogProps {
  assignment: Assignment
  busy: boolean
  error: string | null
  onClose: () => void
  onConfirm: (details: CompletionDetails) => Promise<void>
}

function parseLines(value: string) {
  return value
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
}

export function CompletionDialog({
  assignment,
  busy,
  error,
  onClose,
  onConfirm,
}: CompletionDialogProps) {
  const dialogRef = useRef<HTMLElement>(null)
  const [summary, setSummary] = useState('')
  const [artifactRefs, setArtifactRefs] = useState('')
  const [artifacts, setArtifacts] = useState<PendingArtifact[]>([])
  const [evidenceRefs, setEvidenceRefs] = useState('')
  const [unresolvedBlockers, setUnresolvedBlockers] = useState('')
  const [fileError, setFileError] = useState<string | null>(null)
  useModalDialog({
    canClose: !busy,
    dialogRef,
    onClose,
  })

  useEffect(() => {
    setSummary('')
    setArtifactRefs('')
    setArtifacts([])
    setEvidenceRefs('')
    setUnresolvedBlockers('')
    setFileError(null)
  }, [assignment.id])

  const parsedArtifacts = parseLines(artifactRefs)
  const parsedEvidence = parseLines(evidenceRefs)
  const hasReference =
    parsedArtifacts.length > 0 ||
    artifacts.length > 0 ||
    parsedEvidence.length > 0

  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (!summary.trim() || !hasReference) return
    void onConfirm({
      summary: summary.trim(),
      artifactRefs: parsedArtifacts,
      artifacts,
      evidenceRefs: parsedEvidence,
      unresolvedBlockers: parseLines(unresolvedBlockers),
    })
  }

  const selectArtifacts = (files: FileList | null) => {
    const selected = Array.from(files ?? [])
    if (selected.length > MAX_ARTIFACTS) {
      setFileError(`Select at most ${MAX_ARTIFACTS} artifacts.`)
      return
    }
    const invalid = selected.find(
      (file) =>
        artifactKind(file) === null ||
        file.size === 0 ||
        file.size > MAX_ARTIFACT_BYTES,
    )
    if (invalid) {
      setFileError(`${invalid.name} is not a valid Markdown or HTML artifact.`)
      return
    }
    setFileError(null)
    setArtifacts(
      selected.map((file) => ({
        id: crypto.randomUUID(),
        file,
        kind: artifactKind(file) as ArtifactKind,
      })),
    )
  }

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        aria-labelledby="completion-title"
        aria-modal="true"
        className="control-dialog completion-dialog"
        ref={dialogRef}
        role="dialog"
      >
        <header className="dialog-heading">
          <div>
            <p className="eyebrow">Assignment receipt</p>
            <h2 id="completion-title">Record completion</h2>
          </div>
          <button
            aria-label="Close completion"
            className="icon-button"
            disabled={busy}
            onClick={onClose}
            title="Close"
            type="button"
          >
            <X aria-hidden="true" size={17} />
          </button>
        </header>
        <div className="completion-context">
          <strong>{assignment.profile_name}</strong>
          <span>{assignment.role}</span>
        </div>
        <form className="dialog-form" onSubmit={submit}>
          <label>
            <span>Outcome</span>
            <select aria-readonly="true" disabled value="completed">
              <option value="completed">Completed</option>
            </select>
          </label>
          <label>
            <span>Summary</span>
            <textarea
              autoFocus
              maxLength={16000}
              onChange={(event) => setSummary(event.target.value)}
              required
              rows={4}
              value={summary}
            />
          </label>
          <div className="completion-reference-grid">
            <label>
              <span>Artifact references</span>
              <textarea
                onChange={(event) => setArtifactRefs(event.target.value)}
                rows={3}
                value={artifactRefs}
              />
            </label>
            <label>
              <span>Evidence references</span>
              <textarea
                onChange={(event) => setEvidenceRefs(event.target.value)}
                rows={3}
                value={evidenceRefs}
              />
            </label>
          </div>
          <label>
            <span>Typed artifacts</span>
            <input
              accept=".md,.markdown,.html,.htm,text/markdown,text/html"
              disabled={busy}
              multiple
              onChange={(event) => {
                selectArtifacts(event.target.files)
                event.target.value = ''
              }}
              type="file"
            />
          </label>
          {artifacts.length > 0 ? (
            <ul className="artifact-upload-list">
              {artifacts.map(({ file, id, kind }) => (
                <li key={id}>
                  <FileCode2 aria-hidden="true" size={16} />
                  <span>
                    <strong>{file.name}</strong>
                    <small>
                      {kind} / {formatBytes(file.size)}
                    </small>
                  </span>
                  <button
                    aria-label={`Remove ${file.name}`}
                    className="icon-button"
                    disabled={busy}
                    onClick={() =>
                      setArtifacts((current) =>
                        current.filter((artifact) => artifact.id !== id),
                      )
                    }
                    title="Remove artifact"
                    type="button"
                  >
                    <Trash2 aria-hidden="true" size={15} />
                  </button>
                </li>
              ))}
            </ul>
          ) : null}
          {fileError || error ? (
            <p className="dialog-error" role="alert">
              <CircleAlert aria-hidden="true" size={16} />
              <span>{fileError ?? error}</span>
            </p>
          ) : null}
          <label>
            <span>Unresolved blockers</span>
            <textarea
              onChange={(event) => setUnresolvedBlockers(event.target.value)}
              rows={3}
              value={unresolvedBlockers}
            />
          </label>
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
              disabled={busy || !summary.trim() || !hasReference}
              type="submit"
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
              Record completion
            </button>
          </footer>
        </form>
      </section>
    </div>
  )
}

function artifactKind(file: File): ArtifactKind | null {
  const name = file.name.toLowerCase()
  if (
    file.type === 'text/markdown' ||
    name.endsWith('.md') ||
    name.endsWith('.markdown')
  ) {
    return 'markdown'
  }
  if (
    file.type === 'text/html' ||
    name.endsWith('.html') ||
    name.endsWith('.htm')
  ) {
    return 'html'
  }
  return null
}

function formatBytes(value: number) {
  return value < 1024 ? `${value} B` : `${Math.ceil(value / 1024)} KiB`
}

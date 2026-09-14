import { useEffect, useReducer, useRef, type FormEvent } from 'react'
import { LoaderCircle, Save, X } from 'lucide-react'
import type {
  CreateWorkerProfileInput,
  WorkerProfile,
} from './types'
import {
  createWorkerProfileEditorState,
  workerProfileEditorReducer,
  workerProfilePermissionOptions,
  WORKER_PROFILE_TEMPLATES,
} from './workerProfileTemplates'
import { useModalDialog } from './useModalDialog'

interface ProfileEditorProps {
  busy: boolean
  profile: WorkerProfile | null
  onClose: () => void
  onSave: (spec: CreateWorkerProfileInput) => Promise<void>
}

function listValue(values: string[]) {
  return values.join(', ')
}

function parseList(value: string) {
  return value
    .split(',')
    .map((item) => item.trim())
    .filter(Boolean)
}

export function ProfileEditor({
  busy,
  profile,
  onClose,
  onSave,
}: ProfileEditorProps) {
  const dialogRef = useRef<HTMLElement>(null)
  const [{ spec, templateId }, dispatch] = useReducer(
    workerProfileEditorReducer,
    profile,
    createWorkerProfileEditorState,
  )
  useModalDialog({
    canClose: !busy,
    dialogRef,
    onClose,
  })

  useEffect(() => {
    dispatch({ profile, type: 'reset' })
  }, [profile])

  const submit = (event: FormEvent) => {
    event.preventDefault()
    void onSave(spec)
  }

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        aria-labelledby="profile-editor-title"
        aria-modal="true"
        className="control-dialog profile-editor"
        ref={dialogRef}
        role="dialog"
      >
        <header className="dialog-heading">
          <div>
            <p className="eyebrow">Worker profile</p>
            <h2 id="profile-editor-title">
              {profile ? 'Edit profile' : 'New profile'}
            </h2>
          </div>
          <button
            aria-label="Close profile editor"
            className="icon-button"
            disabled={busy}
            onClick={onClose}
            title="Close"
            type="button"
          >
            <X aria-hidden="true" size={17} />
          </button>
        </header>
        <form className="dialog-form" onSubmit={submit}>
          {!profile ? (
            <label>
              <span>Profile template</span>
              <select
                onChange={(event) => {
                  dispatch({
                    templateId: event.target.value,
                    type: 'select-template',
                  })
                }}
                value={templateId}
              >
                <option value="">Blank profile</option>
                {WORKER_PROFILE_TEMPLATES.map((template) => (
                  <option key={template.id} value={template.id}>
                    {template.label} - {template.description}
                  </option>
                ))}
              </select>
            </label>
          ) : null}
          <div className="form-grid form-grid--identity">
            <label>
              <span>Profile name</span>
              <input
                autoFocus
                maxLength={120}
                onChange={(event) =>
                  dispatch({
                    patch: { name: event.target.value },
                    type: 'update-spec',
                  })
                }
                required
                value={spec.name}
              />
            </label>
            <label>
              <span>Provider</span>
              <select
                onChange={(event) =>
                  dispatch({
                    patch: { provider: event.target.value },
                    type: 'update-spec',
                  })
                }
                value={spec.provider}
              >
                <option value="codex">Codex</option>
                <option value="claude">Claude</option>
                <option value="kiro">Kiro</option>
              </select>
            </label>
            <label>
              <span>Model</span>
              <input
                onChange={(event) =>
                  dispatch({
                    patch: {
                      model: event.target.value.trim() || null,
                    },
                    type: 'update-spec',
                  })
                }
                placeholder="Runtime default"
                value={spec.model ?? ''}
              />
            </label>
            <label>
              <span>Default role</span>
              <input
                onChange={(event) =>
                  dispatch({
                    patch: { default_role: event.target.value },
                    type: 'update-spec',
                  })
                }
                required
                value={spec.default_role}
              />
            </label>
          </div>

          <label>
            <span>Instructions reference</span>
            <input
              onChange={(event) =>
                dispatch({
                  patch: {
                    instructions_ref:
                      event.target.value.trim() || null,
                  },
                  type: 'update-spec',
                })
              }
              placeholder="AGENTS.md"
              value={spec.instructions_ref ?? ''}
            />
          </label>

          <div className="form-grid">
            <label>
              <span>Tools</span>
              <input
                onChange={(event) =>
                  dispatch({
                    patch: { tools: parseList(event.target.value) },
                    type: 'update-spec',
                  })
                }
                value={listValue(spec.tools)}
              />
            </label>
            <label>
              <span>Skills</span>
              <input
                onChange={(event) =>
                  dispatch({
                    patch: { skills: parseList(event.target.value) },
                    type: 'update-spec',
                  })
                }
                value={listValue(spec.skills)}
              />
            </label>
            <label>
              <span>MCP servers</span>
              <input
                onChange={(event) =>
                  dispatch({
                    patch: {
                      mcp_servers: parseList(event.target.value),
                    },
                    type: 'update-spec',
                  })
                }
                value={listValue(spec.mcp_servers)}
              />
            </label>
          </div>

          <div className="form-grid">
            <label>
              <span>Sandbox</span>
              <select
                onChange={(event) =>
                  dispatch({
                    patch: { sandbox_policy: event.target.value },
                    type: 'update-spec',
                  })
                }
                value={spec.sandbox_policy}
              >
                <option value="runtime_default">Runtime default</option>
              </select>
            </label>
            <label>
              <span>Worktree</span>
              <select
                onChange={(event) =>
                  dispatch({
                    patch: { worktree_policy: event.target.value },
                    type: 'update-spec',
                  })
                }
                value={spec.worktree_policy}
              >
                <option value="project_workspace">Project workspace</option>
              </select>
            </label>
            <label>
              <span>Permissions</span>
              <select
                onChange={(event) =>
                  dispatch({
                    patch: { permission_policy: event.target.value },
                    type: 'update-spec',
                  })
                }
                value={spec.permission_policy}
              >
                {workerProfilePermissionOptions(spec.provider).map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>
            </label>
          </div>

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
              disabled={busy || !spec.name.trim() || !spec.default_role.trim()}
              type="submit"
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
              Save profile
            </button>
          </footer>
        </form>
      </section>
    </div>
  )
}

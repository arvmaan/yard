import {
  useEffect,
  useReducer,
  useRef,
  useState,
  type FormEvent,
} from 'react'
import { LoaderCircle, RefreshCw, Save, X } from 'lucide-react'
import { dryRunAgentProfile } from './api'
import type {
  CreateWorkerProfileInput,
  ProfileLaunchPlan,
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

export function ProfileEditor({
  busy,
  profile,
  onClose,
  onSave,
}: ProfileEditorProps) {
  const dialogRef = useRef<HTMLElement>(null)
  const [plan, setPlan] = useState<ProfileLaunchPlan | null>(null)
  const [planError, setPlanError] = useState<string | null>(null)
  const [planLoading, setPlanLoading] = useState(false)
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
    setPlan(null)
    setPlanError(null)
  }, [profile])

  const submit = (event: FormEvent) => {
    event.preventDefault()
    void onSave(spec)
  }

  const previewLaunch = async () => {
    if (!profile) return
    setPlanLoading(true)
    setPlanError(null)
    try {
      setPlan(await dryRunAgentProfile(profile.id, profile.version))
    } catch (caught) {
      setPlanError(
        caught instanceof Error ? caught.message : 'Launch preview failed',
      )
    } finally {
      setPlanLoading(false)
    }
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

          <div className="profile-components" aria-label="Portable components">
            <span>Portable components</span>
            {spec.skills.length > 0 ? (
              <p>Packaged skills: {spec.skills.join(', ')}</p>
            ) : (
              <p>No packaged skills.</p>
            )}
            {spec.tools.length > 0 || spec.mcp_servers.length > 0 ? (
              <p>
                Legacy tool and MCP requests are preserved but must pass the
                launch preview; importing them grants no permissions or
                credentials.
              </p>
            ) : null}
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

          {profile ? (
            <div className="profile-launch-preview">
              <button
                className="secondary-button"
                disabled={busy || planLoading}
                onClick={() => void previewLaunch()}
                type="button"
              >
                {planLoading ? (
                  <LoaderCircle
                    aria-hidden="true"
                    className="status-spin"
                    size={16}
                  />
                ) : (
                  <RefreshCw aria-hidden="true" size={16} />
                )}
                Preview launch plan
              </button>
              {planError ? <p role="alert">{planError}</p> : null}
              {plan ? (
                <div aria-live="polite">
                  <p>
                    {plan.compatible
                      ? 'Launch compatible'
                      : 'Launch blocked'}
                    {' · '}
                    {plan.providerId}
                  </p>
                  <p>
                    {plan.generatedFiles.length} managed file
                    {plan.generatedFiles.length === 1 ? '' : 's'}
                    {plan.approvals.length > 0
                      ? ` · ${plan.approvals.length} approval required`
                      : ''}
                  </p>
                  <ul>
                    {plan.components.map((component) => (
                      <li key={`${component.kind}:${component.id}`}>
                        {component.id}: {component.status}
                      </li>
                    ))}
                  </ul>
                </div>
              ) : null}
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

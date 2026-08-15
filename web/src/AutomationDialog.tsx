import { useId, useMemo, useRef, useState, type FormEvent } from 'react'
import { Clock3, LoaderCircle, Plus, X } from 'lucide-react'
import {
  automationScopeKey,
  automationTargets,
  normalizeAutomationProjectIds,
} from './automationTargets'
import type {
  AutomationSchedule,
  AutomationScope,
  CanvasPlacement,
  CoordinationNode,
  Project,
} from './types'
import { useModalDialog } from './useModalDialog'

export interface AutomationDetails {
  name: string
  scope: AutomationScope
  schedule: AutomationSchedule
  selectedProjectIds: string[]
  promptTemplate: string
}

const COMMON_TIMEZONES = [
  'UTC',
  'America/Los_Angeles',
  'America/Denver',
  'America/Chicago',
  'America/New_York',
  'Europe/London',
  'Europe/Berlin',
  'Asia/Kolkata',
  'Asia/Singapore',
  'Asia/Tokyo',
  'Australia/Sydney',
]

function localTimezone() {
  return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
}

function isValidTimezone(timezone: string) {
  try {
    new Intl.DateTimeFormat('en', { timeZone: timezone }).format()
    return true
  } catch {
    return false
  }
}

export function AutomationDialog({
  busy,
  coordinationNodes,
  error,
  initialScope,
  onClose,
  onCreate,
  placement,
  projects,
}: {
  busy: boolean
  coordinationNodes: CoordinationNode[]
  error: string | null
  initialScope?: AutomationScope
  onClose: () => void
  onCreate: (details: AutomationDetails & { placement: CanvasPlacement }) => void
  placement: CanvasPlacement
  projects: Project[]
}) {
  const titleId = useId()
  const dialogRef = useRef<HTMLFormElement>(null)
  const targets = useMemo(
    () => automationTargets(projects, coordinationNodes),
    [coordinationNodes, projects],
  )
  const initialTarget =
    targets.find(
      (target) =>
        initialScope &&
        target.key === automationScopeKey(initialScope),
    ) ?? targets[0]
  const [name, setName] = useState('')
  const [targetKey, setTargetKey] = useState(initialTarget.key)
  const [hour, setHour] = useState(9)
  const [minute, setMinute] = useState(0)
  const [timezone, setTimezone] = useState(localTimezone)
  const [selectedProjectIds, setSelectedProjectIds] = useState<string[]>(
    initialTarget.projectConstraint === 'exact'
      ? initialTarget.projectIds
      : [],
  )
  const [promptTemplate, setPromptTemplate] = useState('')
  const selectedTarget =
    targets.find((target) => target.key === targetKey) ?? targets[0]
  const scheduleValid =
    Number.isInteger(hour) &&
    hour >= 0 &&
    hour <= 23 &&
    Number.isInteger(minute) &&
    minute >= 0 &&
    minute <= 59
  const timezoneOptions = Array.from(
    new Set([localTimezone(), timezone, ...COMMON_TIMEZONES]),
  )

  useModalDialog({
    canClose: !busy,
    dialogRef,
    onClose,
  })

  const chooseTarget = (nextTargetKey: string) => {
    const nextTarget =
      targets.find((target) => target.key === nextTargetKey) ?? targets[0]
    setTargetKey(nextTarget.key)
    setSelectedProjectIds((current) =>
      normalizeAutomationProjectIds(nextTarget, current),
    )
  }

  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (
      !name.trim() ||
      !promptTemplate.trim() ||
      !scheduleValid ||
      !isValidTimezone(timezone)
    ) {
      return
    }
    onCreate({
      name: name.trim(),
      placement,
      promptTemplate: promptTemplate.trim(),
      schedule: { hour, minute, timezone },
      scope: selectedTarget.scope,
      selectedProjectIds: normalizeAutomationProjectIds(
        selectedTarget,
        selectedProjectIds,
      ),
    })
  }

  return (
    <div className="modal-backdrop automation-dialog-backdrop">
      <form
        aria-labelledby={titleId}
        aria-modal="true"
        className="control-dialog automation-dialog dialog-form"
        onSubmit={submit}
        ref={dialogRef}
        role="dialog"
      >
        <header className="automation-dialog__header">
          <span className="automation-dialog__icon">
            <Clock3 aria-hidden="true" size={19} />
          </span>
          <div>
            <p className="eyebrow">Scheduled dispatch</p>
            <h2 id={titleId}>Create automation</h2>
          </div>
          <button
            aria-label="Close automation creation"
            className="icon-button"
            disabled={busy}
            onClick={onClose}
            title="Close"
            type="button"
          >
            <X aria-hidden="true" size={18} />
          </button>
        </header>

        <label htmlFor="automation-name">
          <span>Name</span>
          <input
            autoFocus
            id="automation-name"
            maxLength={120}
            onChange={(event) => setName(event.target.value)}
            placeholder="Daily release check"
            value={name}
          />
        </label>

        <label htmlFor="automation-target">
          <span>Target orchestrator</span>
          <select
            id="automation-target"
            onChange={(event) => chooseTarget(event.target.value)}
            value={selectedTarget.key}
          >
            {targets.map((target) => (
              <option key={target.key} value={target.key}>
                {target.label}
              </option>
            ))}
          </select>
        </label>

        <div className="automation-schedule-grid">
          <label htmlFor="automation-hour">
            <span>Hour</span>
            <input
              id="automation-hour"
              max={23}
              min={0}
              onChange={(event) => {
                const value = event.currentTarget.valueAsNumber
                if (Number.isFinite(value)) setHour(value)
              }}
              required
              type="number"
              value={hour}
            />
          </label>
          <label htmlFor="automation-minute">
            <span>Minute</span>
            <input
              id="automation-minute"
              max={59}
              min={0}
              onChange={(event) => {
                const value = event.currentTarget.valueAsNumber
                if (Number.isFinite(value)) setMinute(value)
              }}
              required
              type="number"
              value={minute}
            />
          </label>
          <label htmlFor="automation-timezone">
            <span>Timezone</span>
            <input
              aria-invalid={!isValidTimezone(timezone)}
              id="automation-timezone"
              list="automation-timezones"
              onChange={(event) => setTimezone(event.target.value)}
              value={timezone}
            />
            <datalist id="automation-timezones">
              {timezoneOptions.map((option) => (
                <option key={option} value={option} />
              ))}
            </datalist>
          </label>
        </div>

        <fieldset className="automation-project-picker">
          <legend>Projects</legend>
          {selectedTarget.projectIds.length === 0 ? (
            <p className="empty-state">No projects are available for this target.</p>
          ) : (
            projects
              .filter((project) =>
                selectedTarget.projectIds.includes(project.id),
              )
              .map((project) => {
                const exact = selectedTarget.projectConstraint === 'exact'
                return (
                  <label key={project.id}>
                    <input
                      checked={
                        exact || selectedProjectIds.includes(project.id)
                      }
                      disabled={exact}
                      onChange={(event) =>
                        setSelectedProjectIds((current) =>
                          event.target.checked
                            ? [...current, project.id]
                            : current.filter((id) => id !== project.id),
                        )
                      }
                      type="checkbox"
                    />
                    <span>{project.name}</span>
                  </label>
                )
              })
          )}
        </fieldset>

        <label htmlFor="automation-prompt">
          <span>Prompt template</span>
          <textarea
            id="automation-prompt"
            onChange={(event) => setPromptTemplate(event.target.value)}
            placeholder="Review the selected projects and report..."
            rows={7}
            value={promptTemplate}
          />
        </label>

        {!isValidTimezone(timezone) ? (
          <p className="dialog-error" role="alert">
            <span>Enter a valid IANA timezone.</span>
          </p>
        ) : null}
        {error ? (
          <p className="dialog-error" role="alert">
            <span>{error}</span>
          </p>
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
              !name.trim() ||
              !promptTemplate.trim() ||
              !scheduleValid ||
              !isValidTimezone(timezone)
            }
            type="submit"
          >
            {busy ? (
              <LoaderCircle
                aria-label="Creating automation"
                className="status-spin"
                size={16}
              />
            ) : (
              <Plus aria-hidden="true" size={16} />
            )}
            Create automation
          </button>
        </footer>
      </form>
    </div>
  )
}

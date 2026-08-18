import { useMemo, useState } from 'react'
import {
  CircleAlert,
  Clock3,
  LoaderCircle,
  Pause,
  Play,
  RefreshCw,
  Save,
} from 'lucide-react'
import {
  automationScopeKey,
  automationScopeLabel,
  automationTargets,
  normalizeAutomationProjectIds,
} from './automationTargets'
import type { AutomationDetails } from './AutomationDialog'
import type {
  Automation,
  AutomationRun,
  CoordinationNode,
  Project,
} from './types'

const RUN_LABELS = {
  ambiguous: 'Ambiguous',
  failed: 'Failed',
  pending: 'Pending',
  submitted: 'Submitted',
} as const

function formatDate(value: number | null) {
  return value ? new Date(value).toLocaleString() : 'Not scheduled'
}

function formatTime(hour: number, minute: number) {
  return `${String(hour).padStart(2, '0')}:${String(minute).padStart(2, '0')}`
}

function isValidTimezone(timezone: string) {
  try {
    new Intl.DateTimeFormat('en', { timeZone: timezone }).format()
    return true
  } catch {
    return false
  }
}

export function AutomationInspector({
  automation,
  busy,
  coordinationNodes,
  error,
  historyLoading,
  onRun,
  onSave,
  onTogglePaused,
  projects,
  runs,
}: {
  automation: Automation
  busy: boolean
  coordinationNodes: CoordinationNode[]
  error: string | null
  historyLoading: boolean
  onRun: (automation: Automation) => void
  onSave: (automation: Automation, details: AutomationDetails) => void
  onTogglePaused: (automation: Automation) => void
  projects: Project[]
  runs: AutomationRun[]
}) {
  const targets = useMemo(
    () => automationTargets(projects, coordinationNodes),
    [coordinationNodes, projects],
  )
  const fallbackTarget = targets[0]
  const initialTarget =
    targets.find(
      (target) =>
        target.key === automationScopeKey(automation.scope),
    ) ?? fallbackTarget
  const [name, setName] = useState(automation.name)
  const [targetKey, setTargetKey] = useState(initialTarget.key)
  const [hour, setHour] = useState(automation.schedule.hour)
  const [minute, setMinute] = useState(automation.schedule.minute)
  const [timezone, setTimezone] = useState(automation.schedule.timezone)
  const [selectedProjectIds, setSelectedProjectIds] = useState(
    automation.selected_project_ids,
  )
  const [promptTemplate, setPromptTemplate] = useState(
    automation.prompt_template,
  )
  const selectedTarget =
    targets.find((target) => target.key === targetKey) ?? fallbackTarget
  const normalizedProjectIds = normalizeAutomationProjectIds(
    selectedTarget,
    selectedProjectIds,
  )
  const scheduleValid =
    Number.isInteger(hour) &&
    hour >= 0 &&
    hour <= 23 &&
    Number.isInteger(minute) &&
    minute >= 0 &&
    minute <= 59 &&
    isValidTimezone(timezone)
  const dirty =
    name.trim() !== automation.name ||
    targetKey !== automationScopeKey(automation.scope) ||
    hour !== automation.schedule.hour ||
    minute !== automation.schedule.minute ||
    timezone.trim() !== automation.schedule.timezone ||
    promptTemplate.trim() !== automation.prompt_template ||
    normalizedProjectIds.length !== automation.selected_project_ids.length ||
    normalizedProjectIds.some(
      (projectId) => !automation.selected_project_ids.includes(projectId),
    )

  const chooseTarget = (nextTargetKey: string) => {
    const nextTarget =
      targets.find((target) => target.key === nextTargetKey) ?? fallbackTarget
    setTargetKey(nextTarget.key)
    setSelectedProjectIds((current) =>
      normalizeAutomationProjectIds(nextTarget, current),
    )
  }

  return (
    <>
      <div className="inspector__identity">
        <span
          className="inspector__icon automation-inspector__icon"
          data-state={automation.state}
        >
          <Clock3 aria-hidden="true" size={20} />
        </span>
        <div>
          <p className="eyebrow">Automation</p>
          <h2>{automation.name}</h2>
        </div>
      </div>

      <div className="automation-summary">
        <span className="automation-state" data-state={automation.state}>
          {automation.state === 'active' ? 'Active' : 'Paused'}
        </span>
        <span>
          {formatTime(automation.schedule.hour, automation.schedule.minute)}
          {' / '}
          {automation.schedule.timezone}
        </span>
        <small>
          {automationScopeLabel(
            automation.scope,
            projects,
            coordinationNodes,
          )}
        </small>
        <small>Next: {formatDate(automation.next_run_at_unix_ms)}</small>
      </div>

      <section className="automation-settings">
        <label className="field-label" htmlFor="automation-edit-name">
          Name
        </label>
        <input
          id="automation-edit-name"
          maxLength={120}
          onChange={(event) => setName(event.target.value)}
          value={name}
        />

        <label className="field-label" htmlFor="automation-edit-target">
          Target orchestrator
        </label>
        <select
          id="automation-edit-target"
          onChange={(event) => chooseTarget(event.target.value)}
          value={selectedTarget.key}
        >
          {targets.map((target) => (
            <option key={target.key} value={target.key}>
              {target.label}
            </option>
          ))}
        </select>

        <div className="automation-schedule-grid automation-schedule-grid--inspector">
          <label htmlFor="automation-edit-hour">
            <span>Hour</span>
            <input
              id="automation-edit-hour"
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
          <label htmlFor="automation-edit-minute">
            <span>Minute</span>
            <input
              id="automation-edit-minute"
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
          <label htmlFor="automation-edit-timezone">
            <span>Timezone</span>
            <input
              id="automation-edit-timezone"
              onChange={(event) => setTimezone(event.target.value)}
              value={timezone}
            />
          </label>
        </div>

        <fieldset className="automation-project-picker automation-project-picker--inspector">
          <legend>Projects</legend>
          {projects
            .filter((project) => selectedTarget.projectIds.includes(project.id))
            .map((project) => {
              const exact = selectedTarget.projectConstraint === 'exact'
              return (
                <label key={project.id}>
                  <input
                    checked={exact || selectedProjectIds.includes(project.id)}
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
            })}
          {selectedTarget.projectIds.length === 0 ? (
            <p className="empty-state">No projects are available for this target.</p>
          ) : null}
        </fieldset>

        <label className="field-label" htmlFor="automation-edit-prompt">
          Prompt template
        </label>
        <textarea
          id="automation-edit-prompt"
          onChange={(event) => setPromptTemplate(event.target.value)}
          rows={6}
          value={promptTemplate}
        />

        <button
          className="secondary-button"
          disabled={
            busy ||
            !dirty ||
            !name.trim() ||
            !promptTemplate.trim() ||
            !scheduleValid
          }
          onClick={() =>
            onSave(automation, {
              name: name.trim(),
              promptTemplate: promptTemplate.trim(),
              schedule: {
                hour,
                minute,
                timezone: timezone.trim(),
              },
              scope: selectedTarget.scope,
              selectedProjectIds: normalizedProjectIds,
            })
          }
          type="button"
        >
          {busy ? (
            <LoaderCircle aria-hidden="true" className="status-spin" size={15} />
          ) : (
            <Save aria-hidden="true" size={15} />
          )}
          Save automation
        </button>
      </section>

      {error ? (
        <p className="dialog-error automation-inspector__error" role="alert">
          <CircleAlert aria-hidden="true" size={16} />
          <span>{error}</span>
        </p>
      ) : null}

      <section className="inspector-actions automation-actions">
        <p className="eyebrow">Automatic schedule</p>
        <button
          className="secondary-button"
          disabled={busy}
          onClick={() => onTogglePaused(automation)}
          type="button"
        >
          {automation.state === 'active' ? (
            <Pause aria-hidden="true" size={15} />
          ) : (
            <Play aria-hidden="true" size={15} />
          )}
          {automation.state === 'active' ? 'Pause schedule' : 'Resume schedule'}
        </button>
        <p className="eyebrow automation-actions__manual">Manual dispatch</p>
        <button
          className="command-button"
          disabled={busy}
          onClick={() => onRun(automation)}
          type="button"
        >
          {busy ? (
            <LoaderCircle aria-hidden="true" className="status-spin" size={16} />
          ) : (
            <RefreshCw aria-hidden="true" size={16} />
          )}
          Run now
        </button>
      </section>

      <section className="automation-history">
        <div className="automation-history__heading">
          <div>
            <p className="eyebrow">Run history</p>
            <strong>{runs.length} runs</strong>
          </div>
          {historyLoading ? (
            <LoaderCircle
              aria-label="Loading automation history"
              className="status-spin"
              size={16}
            />
          ) : null}
        </div>
        {runs.length === 0 && !historyLoading ? (
          <p className="empty-state">No runs have been requested.</p>
        ) : (
          <div className="automation-history__list">
            {runs.map((run) => (
              <article
                className="automation-run"
                data-status={run.status}
                key={run.id}
              >
                <span className="automation-run__status">
                  {RUN_LABELS[run.status]}
                </span>
                <div>
                  <strong>
                    {run.trigger === 'manual' ? 'Manual run' : 'Scheduled run'}
                  </strong>
                  <small>{formatDate(run.created_at_unix_ms)}</small>
                  {run.status === 'submitted' ? (
                    <small>Transport accepted. Completion is not implied.</small>
                  ) : null}
                  {run.runtime_status ? (
                    <code>{run.runtime_status}</code>
                  ) : null}
                  {run.error_message ? (
                    <p>{run.error_message}</p>
                  ) : null}
                </div>
              </article>
            ))}
          </div>
        )}
      </section>
    </>
  )
}

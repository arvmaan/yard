import { useEffect, useState } from 'react'
import {
  CircleAlert,
  Database,
  FolderArchive,
  LoaderCircle,
  Network,
  RefreshCw,
  Save,
} from 'lucide-react'
import { WorkerInterventions } from './WorkerInterventions'
import type {
  CoordinationNode,
  CoordinationNodeRoute,
  CoordinationSnapshot,
  ObservedStatus,
  Project,
  WorkerProfile,
} from './types'

export function CoordinationNodeInspector({
  busy,
  node,
  onProvision,
  onRouteChange,
  onSnapshot,
  onUpdate,
  profiles,
  projects,
  routes,
  snapshots,
}: {
  busy: boolean
  node: CoordinationNode
  onProvision: (node: CoordinationNode, profile: WorkerProfile) => void
  onRouteChange: (route: CoordinationNodeRoute) => void
  onSnapshot: (node: CoordinationNode) => void
  onUpdate: (
    node: CoordinationNode,
    name: string,
    attachedProjectIds: string[],
  ) => void
  profiles: WorkerProfile[]
  projects: Project[]
  routes: CoordinationNodeRoute[]
  snapshots: CoordinationSnapshot[]
}) {
  const [name, setName] = useState(node.name)
  const [attachedProjectIds, setAttachedProjectIds] = useState(
    node.attached_project_ids,
  )
  const eligibleProfiles = profiles.filter(
    (profile) => profile.runtime_adapter === 'herdr',
  )
  const [profileId, setProfileId] = useState(
    eligibleProfiles.find(
      (profile) => profile.default_role === 'orchestrator',
    )?.id ??
      eligibleProfiles[0]?.id ??
      '',
  )
  const Icon = node.kind === 'workstream' ? Network : Database
  const attachedProjects = projects.filter((project) =>
    node.attached_project_ids.includes(project.id),
  )
  const workerStatus: ObservedStatus =
    node.worker?.runtime?.status ?? 'unknown'
  const dirty =
    name.trim() !== node.name ||
    attachedProjectIds.length !== node.attached_project_ids.length ||
    attachedProjectIds.some(
      (projectId) => !node.attached_project_ids.includes(projectId),
    )

  useEffect(() => {
    setName(node.name)
    setAttachedProjectIds(node.attached_project_ids)
  }, [node])

  const selectedProfile = eligibleProfiles.find(
    (profile) => profile.id === profileId,
  )

  return (
    <>
      <div className="inspector__identity">
        <span
          className="inspector__icon coordination-inspector__icon"
          data-kind={node.kind}
          data-status={workerStatus}
        >
          <Icon aria-hidden="true" size={20} />
        </span>
        <div>
          <p className="eyebrow">
            {node.kind === 'workstream'
              ? 'Workstream orchestrator'
              : 'Knowledge store'}
          </p>
          <h2>{node.name}</h2>
        </div>
      </div>

      <div className="coordination-node-state">
        <span className="runtime-badge" data-runtime={node.worker ? 'online' : 'offline'}>
          {node.kind === 'knowledge_store'
            ? `${node.attached_project_ids.length} projects`
            : node.worker
              ? workerStatus
              : 'not provisioned'}
        </span>
        <code>v{node.version}</code>
      </div>

      <section className="coordination-node-settings">
        <label className="field-label" htmlFor="coordination-node-edit-name">
          Name
        </label>
        <input
          id="coordination-node-edit-name"
          maxLength={120}
          onChange={(event) => setName(event.target.value)}
          value={name}
        />
        <fieldset className="coordination-project-picker coordination-project-picker--inspector">
          <legend>Attached projects</legend>
          {projects.map((project) => (
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
          ))}
        </fieldset>
        <button
          className="secondary-button"
          disabled={busy || !dirty || !name.trim()}
          onClick={() => onUpdate(node, name.trim(), attachedProjectIds)}
          type="button"
        >
          {busy ? (
            <LoaderCircle
              aria-hidden="true"
              className="status-spin"
              size={15}
            />
          ) : (
            <Save aria-hidden="true" size={15} />
          )}
          Save node
        </button>
      </section>

      {node.kind === 'workstream' ? (
        node.worker ? (
          <WorkerInterventions
            key={`${node.id}:${node.version}:${node.worker.id}`}
            onCoordinationNodeChange={onRouteChange}
            projects={attachedProjects}
            routes={routes}
            status={workerStatus}
            target={{ kind: 'coordination-node', node }}
          />
        ) : (
          <section className="inspector-actions coordination-provision">
            <div className="awaiting-disposition" role="status">
              <CircleAlert aria-hidden="true" size={16} />
              <span>
                <strong>No orchestrator worker</strong>
                <small>Provision a dedicated worker for this workstream.</small>
              </span>
            </div>
            <label className="field-label" htmlFor="coordination-node-provision-profile">
              Worker profile
            </label>
            <select
              disabled={busy || eligibleProfiles.length === 0}
              id="coordination-node-provision-profile"
              onChange={(event) => setProfileId(event.target.value)}
              value={profileId}
            >
              {eligibleProfiles.map((profile) => (
                <option key={profile.id} value={profile.id}>
                  {profile.name} / {profile.provider}
                </option>
              ))}
            </select>
            <button
              className="command-button"
              disabled={busy || !selectedProfile}
              onClick={() => {
                if (selectedProfile) onProvision(node, selectedProfile)
              }}
              type="button"
            >
              {busy ? (
                <LoaderCircle
                  aria-hidden="true"
                  className="status-spin"
                  size={16}
                />
              ) : (
                <Network aria-hidden="true" size={16} />
              )}
              Start orchestrator
            </button>
          </section>
        )
      ) : (
        <section className="knowledge-snapshots">
          <div className="knowledge-snapshots__heading">
            <div>
              <p className="eyebrow">Knowledge snapshots</p>
              <strong>{snapshots.length} revisions</strong>
            </div>
            <button
              aria-label="Request knowledge snapshot"
              className="icon-button"
              disabled={busy || node.attached_project_ids.length === 0}
              onClick={() => onSnapshot(node)}
              title="Collect project knowledge"
              type="button"
            >
              {busy ? (
                <LoaderCircle
                  aria-hidden="true"
                  className="status-spin"
                  size={16}
                />
              ) : (
                <RefreshCw aria-hidden="true" size={16} />
              )}
            </button>
          </div>
          {snapshots.length === 0 ? (
            <p className="empty-state">
              Attach projects, then collect the first snapshot.
            </p>
          ) : (
            snapshots.map((snapshot) => (
              <article className="knowledge-snapshot" key={snapshot.id}>
                <span>
                  <FolderArchive aria-hidden="true" size={16} />
                </span>
                <div>
                  <strong>
                    {snapshot.progress.completed}/{snapshot.progress.total}{' '}
                    collected
                  </strong>
                  <small>
                    {new Date(snapshot.created_at_unix_ms).toLocaleString()}
                  </small>
                  <code title={snapshot.folder_path}>
                    {snapshot.folder_path}
                  </code>
                </div>
              </article>
            ))
          )}
        </section>
      )}
    </>
  )
}

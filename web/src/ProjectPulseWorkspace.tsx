import { useEffect, useId, useRef } from 'react'
import {
  Activity,
  ArrowRight,
  Database,
  Network,
  X,
} from 'lucide-react'
import { YardOrchestratorOverview } from './YardOrchestratorOverview'
import type { ProjectStatusReports } from './projectUpdates'
import type {
  Assignment,
  CoordinationNode,
  CoordinationSnapshot,
  Project,
  RuntimeInventory,
  YardOrchestratorRoute,
} from './types'

export function ProjectPulseWorkspace({
  assignments,
  inventory,
  onClose,
  onOpenCoordinationNode,
  onOpenProject,
  coordinationNodes,
  coordinationSnapshots,
  projects,
  returnFocus,
  routes,
  statusReports,
}: {
  assignments: Assignment[]
  inventory: RuntimeInventory | null
  onClose: () => void
  onOpenCoordinationNode: (node: CoordinationNode) => void
  onOpenProject: (project: Project) => void
  coordinationNodes: CoordinationNode[]
  coordinationSnapshots: Record<string, CoordinationSnapshot[]>
  projects: Project[]
  returnFocus: HTMLElement | null
  routes: YardOrchestratorRoute[]
  statusReports: ProjectStatusReports
}) {
  const titleId = useId()
  const workspace = useRef<HTMLElement>(null)
  const closeButton = useRef<HTMLButtonElement>(null)
  const closing = useRef(false)
  const onCloseRef = useRef(onClose)
  const returnFocusRef = useRef(returnFocus)
  onCloseRef.current = onClose
  returnFocusRef.current = returnFocus

  useEffect(() => {
    const frame = window.requestAnimationFrame(() =>
      closeButton.current?.focus(),
    )
    return () => window.cancelAnimationFrame(frame)
  }, [])

  useEffect(() => {
    const background = document.querySelector<HTMLElement>('.app-shell')
    if (!background) return

    const hadInert = background.hasAttribute('inert')
    const previousAriaHidden = background.getAttribute('aria-hidden')
    background.inert = true
    background.setAttribute('aria-hidden', 'true')

    return () => {
      if (!hadInert) background.inert = false
      if (previousAriaHidden === null) {
        background.removeAttribute('aria-hidden')
      } else {
        background.setAttribute('aria-hidden', previousAriaHidden)
      }
    }
  }, [])

  useEffect(() => {
    const requestClose = () => {
      if (closing.current) return
      closing.current = true
      const focusTarget = returnFocusRef.current
      onCloseRef.current()
      window.setTimeout(() => focusTarget?.focus(), 0)
    }
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault()
        requestClose()
        return
      }
      if (event.key !== 'Tab' || !workspace.current) return

      const focusable = Array.from(
        workspace.current.querySelectorAll<HTMLElement>(
          'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
        ),
      ).filter(
        (element) =>
          element.getAttribute('aria-hidden') !== 'true' &&
          element.getClientRects().length > 0,
      )
      const first = focusable[0]
      const last = focusable.at(-1)
      if (!first || !last) {
        event.preventDefault()
        workspace.current.focus()
        return
      }

      const active = document.activeElement
      if (
        event.shiftKey &&
        (active === first || !workspace.current.contains(active))
      ) {
        event.preventDefault()
        last.focus()
      } else if (
        !event.shiftKey &&
        (active === last || !workspace.current.contains(active))
      ) {
        event.preventDefault()
        first.focus()
      }
    }
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [])

  const requestClose = () => {
    if (closing.current) return
    closing.current = true
    const focusTarget = returnFocusRef.current
    onCloseRef.current()
    window.setTimeout(() => focusTarget?.focus(), 0)
  }

  return (
    <div className="modal-backdrop pulse-backdrop" role="presentation">
      <section
        aria-labelledby={titleId}
        aria-modal="true"
        className="pulse-workspace"
        ref={workspace}
        role="dialog"
        tabIndex={-1}
      >
        <header className="pulse-workspace__header">
          <span className="pulse-workspace__icon">
            <Activity aria-hidden="true" size={20} />
          </span>
          <div>
            <p className="eyebrow">Portfolio status</p>
            <h2 id={titleId}>Project pulse</h2>
          </div>
          <button
            aria-label="Close Project pulse"
            className="icon-button"
            onClick={requestClose}
            ref={closeButton}
            title="Close Project pulse"
            type="button"
          >
            <X aria-hidden="true" size={18} />
          </button>
        </header>
        <div className="pulse-workspace__body">
          <YardOrchestratorOverview
            assignments={assignments}
            inventory={inventory}
            onOpenProject={onOpenProject}
            projects={projects}
            routes={routes}
            statusReports={statusReports}
          />
          {coordinationNodes.length > 0 ? (
            <section
              aria-label="Coordination nodes"
              className="pulse-coordination"
            >
              <div className="pulse-coordination__heading">
                <div>
                  <p className="eyebrow">Shared infrastructure</p>
                  <h3>Coordination</h3>
                </div>
                <span>{coordinationNodes.length}</span>
              </div>
              <div className="pulse-coordination__list">
                {coordinationNodes.map((node) => {
                  const Icon =
                    node.kind === 'workstream' ? Network : Database
                  const latestSnapshot =
                    coordinationSnapshots[node.id]?.[0]
                  return (
                    <button
                      key={node.id}
                      onClick={() => onOpenCoordinationNode(node)}
                      type="button"
                    >
                      <span data-kind={node.kind}>
                        <Icon aria-hidden="true" size={16} />
                      </span>
                      <span>
                        <strong>{node.name}</strong>
                        <small>
                          {node.kind === 'workstream'
                            ? `${node.attached_project_ids.length} projects / ${
                                node.worker
                                  ? node.worker.runtime?.status ?? 'unknown'
                                  : 'needs worker'
                              }`
                            : latestSnapshot
                              ? `${latestSnapshot.progress.completed}/${latestSnapshot.progress.total} collected`
                              : `${node.attached_project_ids.length} projects / no snapshot`}
                        </small>
                      </span>
                      <ArrowRight aria-hidden="true" size={15} />
                    </button>
                  )
                })}
              </div>
            </section>
          ) : null}
        </div>
      </section>
    </div>
  )
}

import type {
  AutomationScope,
  CoordinationNode,
  Project,
} from './types'

export type AutomationProjectConstraint = 'any' | 'exact' | 'subset'

export interface AutomationTarget {
  key: string
  label: string
  scope: AutomationScope
  projectConstraint: AutomationProjectConstraint
  projectIds: string[]
}

export function automationScopeKey(scope: AutomationScope) {
  if (scope.kind === 'project_orchestrator') {
    return `project:${scope.project_id}`
  }
  if (scope.kind === 'workstream_coordination_node') {
    return `workstream:${scope.node_id}`
  }
  return 'superintendent'
}

export function automationTargets(
  projects: Project[],
  coordinationNodes: CoordinationNode[],
): AutomationTarget[] {
  return [
    {
      key: 'superintendent',
      label: 'Superintendent',
      scope: { kind: 'yard_orchestrator' },
      projectConstraint: 'any',
      projectIds: projects.map((project) => project.id),
    },
    ...projects.map(
      (project): AutomationTarget => ({
        key: `project:${project.id}`,
        label: `${project.name} orchestrator`,
        scope: {
          kind: 'project_orchestrator',
          project_id: project.id,
        },
        projectConstraint: 'exact',
        projectIds: [project.id],
      }),
    ),
    ...coordinationNodes.flatMap((node): AutomationTarget[] =>
      node.kind === 'workstream'
        ? [
            {
              key: `workstream:${node.id}`,
              label: node.name,
              scope: {
                kind: 'workstream_coordination_node',
                node_id: node.id,
              },
              projectConstraint: 'subset',
              projectIds: [...node.attached_project_ids],
            },
          ]
        : [],
    ),
  ]
}

export function findAutomationTarget(
  scope: AutomationScope,
  projects: Project[],
  coordinationNodes: CoordinationNode[],
) {
  const key = automationScopeKey(scope)
  return automationTargets(projects, coordinationNodes).find(
    (target) => target.key === key,
  )
}

export function normalizeAutomationProjectIds(
  target: AutomationTarget,
  selectedProjectIds: string[],
) {
  if (target.projectConstraint === 'exact') return [...target.projectIds]
  const allowed = new Set(target.projectIds)
  return selectedProjectIds.filter((projectId, index, values) => {
    return allowed.has(projectId) && values.indexOf(projectId) === index
  })
}

export function automationScopeLabel(
  scope: AutomationScope,
  projects: Project[],
  coordinationNodes: CoordinationNode[],
) {
  return (
    findAutomationTarget(scope, projects, coordinationNodes)?.label ??
    'Unavailable target'
  )
}

import type { Project, RuntimeInventory } from './types'

// Prefilled working directory for a replacement orchestrator: the project
// workspace's worktree checkout, or empty. Pane directories are never used:
// after a reboot the workspace's panes may be reused or unrelated, and the
// replacement must not silently start in one of their directories. Any other
// directory has to be entered by the operator.
export function replacementWorkingDirectoryDefault(
  project: Pick<Project, 'runtime'>,
  inventory: RuntimeInventory | null,
): string {
  if (
    !inventory ||
    inventory.adapter !== project.runtime.adapter ||
    inventory.session !== project.runtime.session
  ) {
    return ''
  }
  const workspaceId = project.runtime.workspace_id
  const checkout = inventory.workspaces.find(
    (workspace) => workspace.runtime_id === workspaceId,
  )?.worktree?.checkout_path
  return checkout ?? ''
}

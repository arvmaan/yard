export interface OrderTemplate {
  id: string
  label: string
  text: string
}

export const MANUAL_INTERVENTION_CONTRACT = `Exit contract:
If you cannot continue without human input, stop and end with:
MANUAL INTERVENTION
Trying: <what you were trying to do>
Why: <why this work matters>
Blocked by: <the exact blocker>
Need from you: <the smallest decision, credential, or input required>`

export const ORDER_TEMPLATES: readonly OrderTemplate[] = [
  {
    id: 'pick-up-work',
    label: 'Pick up work',
    text: `Objective: Pick up the highest-priority unfinished work in your current assignment.
Done when: The next coherent slice is implemented and its concrete result is reported.
Verify: Inspect the current state first, then run the focused checks appropriate to the change.
Execution: Continue autonomously until the objective is done or the exit contract applies.`,
  },
  {
    id: 'status-check',
    label: 'Status check',
    text: `Objective: Give a concise operational status and continue useful work.
Report: Current objective, completed work, current action, blockers, and next step.
Verify: Base the report on the repository and runtime state, not assumptions.
Execution: Continue working after the report unless the exit contract applies.`,
  },
  {
    id: 'resolve-blocker',
    label: 'Resolve blocker',
    text: `Objective: Re-examine and resolve the current blocker.
Done when: Work is moving again or the exact irreducible blocker is identified.
Verify: Try the safest concrete path around the blocker and validate the result.
Execution: Request manual intervention only for a specific decision, credential, or access grant.`,
  },
  {
    id: 'completion-handoff',
    label: 'Prepare completion',
    text: `Objective: Prepare an evidence-backed completion handoff for your current assignment.
Report: Concise result, changed files or artifacts, verification commands and outcomes, and any unresolved blockers.
Done when: The operator has enough concrete evidence to review the result and record the Yard completion receipt.
Execution: Do not end the session; remain available for review or follow-up.`,
  },
  {
    id: 'intervention-brief',
    label: 'Intervention brief',
    text: `Objective: Stop execution and prepare a manual-intervention brief now.
Done when: The brief identifies the attempted work, its purpose, the exact blocker, and the smallest input needed.
Verify: Make the request specific enough for the user to answer in one response.`,
  },
]

export function buildExecutionOrder(text: string) {
  return `${text.trim()}\n\n${MANUAL_INTERVENTION_CONTRACT}`
}

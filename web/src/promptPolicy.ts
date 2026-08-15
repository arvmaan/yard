import { YardApiError } from './api'

const DURABLE_PROMPT_FAILURE_CODES = new Set([
  'runtime_intervention_ambiguous',
  'command_outcome_ambiguous',
  'command_previously_failed',
])

export function promptRequiresNewCommand(caught: unknown) {
  return (
    caught instanceof YardApiError &&
    DURABLE_PROMPT_FAILURE_CODES.has(caught.code)
  )
}

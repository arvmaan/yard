/**
 * Whether one-click Complete also ends the worker's session. Ending the
 * session ends Yard's tracking and queues runtime cleanup; Yard never closes
 * the Herdr tab, so the agent keeps running until the tab is closed.
 */
export const COMPLETE_AND_END_SESSION_KEY = 'yard:complete-and-end-session:v1'

export function readCompleteAndEndSession(): boolean {
  try {
    return localStorage.getItem(COMPLETE_AND_END_SESSION_KEY) !== 'false'
  } catch {
    return true
  }
}

export function writeCompleteAndEndSession(enabled: boolean) {
  try {
    localStorage.setItem(
      COMPLETE_AND_END_SESSION_KEY,
      enabled ? 'true' : 'false',
    )
  } catch {
    // The preference stays in memory when storage is unavailable.
  }
}

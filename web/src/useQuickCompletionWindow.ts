import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { QUICK_COMPLETE_UNDO_MS } from './assignmentDisposition'
import type { Assignment } from './types'

export interface PendingQuickCompletion {
  assignment: Assignment
  deadline: number
  endSession: boolean
}

/**
 * The pre-commit Undo window of one-click Complete. Nothing is sent until
 * the window ends, so Undo simply never sends the request. A completion
 * still inside its window is dropped when the page is hidden or unloaded,
 * which is the safe direction: the work stays active.
 */
export function useQuickCompletionWindow({
  onDropped,
  onSend,
  windowMs = QUICK_COMPLETE_UNDO_MS,
}: {
  onDropped: () => void
  onSend: (pending: PendingQuickCompletion) => void
  windowMs?: number
}) {
  const [pending, setPending] = useState<
    Record<string, PendingQuickCompletion>
  >({})
  const pendingRef = useRef(pending)
  const onSendRef = useRef(onSend)
  const onDroppedRef = useRef(onDropped)

  useEffect(() => {
    pendingRef.current = pending
    onSendRef.current = onSend
    onDroppedRef.current = onDropped
  })

  const start = useCallback(
    (assignment: Assignment, endSession: boolean) => {
      setPending((current) => ({
        ...current,
        [assignment.id]: {
          assignment,
          deadline: Date.now() + windowMs,
          endSession,
        },
      }))
    },
    [windowMs],
  )

  const undo = useCallback((assignmentId: string) => {
    setPending((current) => {
      if (!(assignmentId in current)) return current
      const next = { ...current }
      delete next[assignmentId]
      return next
    })
  }, [])

  useEffect(() => {
    const timers = Object.entries(pending).map(([assignmentId, entry]) =>
      window.setTimeout(
        () => {
          undo(assignmentId)
          onSendRef.current(entry)
        },
        Math.max(0, entry.deadline - Date.now()),
      ),
    )
    return () => {
      for (const timer of timers) window.clearTimeout(timer)
    }
  }, [pending, undo])

  useEffect(() => {
    const drop = () => {
      if (Object.keys(pendingRef.current).length === 0) return
      setPending({})
      onDroppedRef.current()
    }
    const onVisibilityChange = () => {
      if (document.visibilityState === 'hidden') drop()
    }
    document.addEventListener('visibilitychange', onVisibilityChange)
    window.addEventListener('pagehide', drop)
    return () => {
      document.removeEventListener('visibilitychange', onVisibilityChange)
      window.removeEventListener('pagehide', drop)
    }
  }, [])

  const isPending = useCallback(
    (assignmentId: string) => assignmentId in pending,
    [pending],
  )

  const pendingCompletions = useMemo(() => Object.values(pending), [pending])

  return { isPending, pendingCompletions, start, undo }
}

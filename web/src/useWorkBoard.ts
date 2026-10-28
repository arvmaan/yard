import { useCallback, useEffect, useRef, useState } from 'react'
import { YardApiError, fetchWorkBoard, generateWorkBoard, triageWorkBoard } from './api'
import { isUnchanged, type WorkBoard } from './workBoard'

export const WORK_BOARD_POLL_MS = 1_500

function message(caught: unknown, fallback: string) {
  if (caught instanceof YardApiError) return caught.message
  return caught instanceof Error ? caught.message : fallback
}

/**
 * The cached board: read when `active` turns on, polled every 1.5 s while a
 * generation runs (with `since`, so unchanged polls are tiny), never polled
 * otherwise.
 */
export function useWorkBoard(active: boolean) {
  const [board, setBoard] = useState<WorkBoard | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [generateError, setGenerateError] = useState<string | null>(null)
  const [locallyChanged, setLocallyChanged] = useState(false)
  const revision = useRef<number | undefined>(undefined)
  const inFlight = useRef(false)

  const load = useCallback(async () => {
    if (inFlight.current) return
    inFlight.current = true
    try {
      const response = await fetchWorkBoard(revision.current)
      setError(null)
      if (isUnchanged(response)) return
      revision.current = response.revision
      setBoard(response)
    } catch (caught) {
      setError(message(caught, 'Unable to read the work board'))
    } finally {
      inFlight.current = false
    }
  }, [])

  useEffect(() => {
    if (active) void load()
  }, [active, load])

  const running = board?.state === 'running'
  useEffect(() => {
    if (!running) return
    const timer = window.setInterval(() => void load(), WORK_BOARD_POLL_MS)
    return () => window.clearInterval(timer)
  }, [running, load])

  const generate = useCallback(
    async (useModel: boolean) => {
      setGenerateError(null)
      try {
        await generateWorkBoard(useModel)
        setLocallyChanged(false)
        await load()
      } catch (caught) {
        setGenerateError(message(caught, 'Unable to refresh the work board'))
      }
    },
    [load],
  )

  /**
   * Record a Keep / Skip / Clear on one card. The card moves at once
   * (optimistic) and the server's re-placed board is read back.
   */
  const triage = useCallback(
    async (itemId: string, choice: 'keep' | 'skip' | 'clear') => {
      setBoard((current) => {
        if (!current?.items) return current
        return {
          ...current,
          items: current.items.map((item) =>
            item.id !== itemId
              ? item
              : {
                  ...item,
                  pinned: choice === 'keep',
                  reviewed: choice === 'skip',
                  relevance:
                    choice === 'keep'
                      ? item.relevance === 'older'
                        ? 'pinned'
                        : item.relevance
                      : item.relevance === 'pinned'
                        ? 'older'
                        : item.relevance,
                },
          ),
        }
      })
      try {
        await triageWorkBoard(itemId, choice)
        setError(null)
        await load()
      } catch (caught) {
        setError(message(caught, 'Unable to save that choice'))
        await load()
      }
    },
    [load],
  )

  return {
    board,
    error,
    generate,
    generateError,
    triage,
    locallyChanged,
    setBoard,
    setLocallyChanged,
  }
}

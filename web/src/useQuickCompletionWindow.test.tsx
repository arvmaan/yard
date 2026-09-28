// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, renderHook } from '@testing-library/react'
import { useQuickCompletionWindow } from './useQuickCompletionWindow'
import { activeAssignmentFixture } from './testSupport/assignmentFixture'

beforeEach(() => {
  vi.useFakeTimers()
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('useQuickCompletionWindow', () => {
  it('sends a completion only when its five-second window ends', () => {
    const onSend = vi.fn()
    const { result } = renderHook(() =>
      useQuickCompletionWindow({ onDropped: vi.fn(), onSend }),
    )
    const assignment = activeAssignmentFixture()

    act(() => result.current.start(assignment, true))
    expect(result.current.isPending(assignment.id)).toBe(true)
    act(() => {
      vi.advanceTimersByTime(4_999)
    })
    expect(onSend).not.toHaveBeenCalled()
    act(() => {
      vi.advanceTimersByTime(1)
    })
    expect(onSend).toHaveBeenCalledTimes(1)
    expect(onSend).toHaveBeenCalledWith(
      expect.objectContaining({ assignment, endSession: true }),
    )
    expect(result.current.isPending(assignment.id)).toBe(false)
    act(() => {
      vi.advanceTimersByTime(10_000)
    })
    expect(onSend).toHaveBeenCalledTimes(1)
  })

  it('never sends a completion that was undone', () => {
    const onSend = vi.fn()
    const { result } = renderHook(() =>
      useQuickCompletionWindow({ onDropped: vi.fn(), onSend }),
    )
    const assignment = activeAssignmentFixture()

    act(() => result.current.start(assignment, false))
    act(() => {
      vi.advanceTimersByTime(3_000)
    })
    act(() => result.current.undo(assignment.id))
    act(() => {
      vi.advanceTimersByTime(10_000)
    })
    expect(onSend).not.toHaveBeenCalled()
    expect(result.current.isPending(assignment.id)).toBe(false)
  })

  it('drops a pending completion when the page is hidden', () => {
    const onSend = vi.fn()
    const onDropped = vi.fn()
    const { result } = renderHook(() =>
      useQuickCompletionWindow({ onDropped, onSend }),
    )
    act(() => result.current.start(activeAssignmentFixture(), true))
    act(() => {
      window.dispatchEvent(new Event('pagehide'))
    })
    act(() => {
      vi.advanceTimersByTime(10_000)
    })
    expect(onSend).not.toHaveBeenCalled()
    expect(onDropped).toHaveBeenCalledTimes(1)
  })

  it('drops a pending completion when the tab becomes hidden, not while visible', () => {
    const onSend = vi.fn()
    const onDropped = vi.fn()
    const visibility = vi
      .spyOn(document, 'visibilityState', 'get')
      .mockReturnValue('visible')
    const { result } = renderHook(() =>
      useQuickCompletionWindow({ onDropped, onSend }),
    )
    const assignment = activeAssignmentFixture()
    try {
      act(() => result.current.start(assignment, true))
      act(() => {
        document.dispatchEvent(new Event('visibilitychange'))
      })
      expect(result.current.isPending(assignment.id)).toBe(true)
      visibility.mockReturnValue('hidden')
      act(() => {
        document.dispatchEvent(new Event('visibilitychange'))
      })
      expect(result.current.isPending(assignment.id)).toBe(false)
      act(() => {
        vi.advanceTimersByTime(10_000)
      })
      expect(onSend).not.toHaveBeenCalled()
      expect(onDropped).toHaveBeenCalledTimes(1)
    } finally {
      visibility.mockRestore()
    }
  })
})

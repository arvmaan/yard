import { describe, expect, it } from 'vitest'
import { createLatestFrameQueue } from './latestFrameQueue'

function frameHarness() {
  const callbacks = new Map<number, FrameRequestCallback>()
  const cancelled: number[] = []
  let nextHandle = 1
  return {
    callbacks,
    cancelled,
    request(callback: FrameRequestCallback) {
      const handle = nextHandle
      nextHandle += 1
      callbacks.set(handle, callback)
      return handle
    },
    cancel(handle: number) {
      cancelled.push(handle)
      callbacks.delete(handle)
    },
  }
}

describe('createLatestFrameQueue', () => {
  it('coalesces a burst into one frame and applies only the latest value', () => {
    const frames = frameHarness()
    const applied: number[] = []
    const queue = createLatestFrameQueue(
      (value: number) => applied.push(value),
      frames.request,
      frames.cancel,
    )

    queue.schedule(1)
    queue.schedule(2)
    queue.schedule(3)

    expect(frames.callbacks.size).toBe(1)
    frames.callbacks.get(1)?.(0)
    expect(applied).toEqual([3])
  })

  it('allows another update after the queued frame runs', () => {
    const frames = frameHarness()
    const applied: number[] = []
    const queue = createLatestFrameQueue(
      (value: number) => applied.push(value),
      frames.request,
      frames.cancel,
    )

    queue.schedule(1)
    frames.callbacks.get(1)?.(0)
    queue.schedule(2)

    expect(frames.callbacks.has(2)).toBe(true)
    frames.callbacks.get(2)?.(0)
    expect(applied).toEqual([1, 2])
  })

  it('flushes the latest value and cancels the pending frame', () => {
    const frames = frameHarness()
    const applied: number[] = []
    const queue = createLatestFrameQueue(
      (value: number) => applied.push(value),
      frames.request,
      frames.cancel,
    )

    queue.schedule(1)
    queue.schedule(2)
    queue.flush()

    expect(frames.cancelled).toEqual([1])
    expect(applied).toEqual([2])
  })
})

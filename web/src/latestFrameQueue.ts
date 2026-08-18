export interface LatestFrameQueue<T> {
  cancel: () => void
  flush: () => void
  schedule: (value: T) => void
}

export function createLatestFrameQueue<T>(
  apply: (value: T) => void,
  requestFrame: (callback: FrameRequestCallback) => number = (callback) =>
    window.requestAnimationFrame(callback),
  cancelFrame: (handle: number) => void = (handle) =>
    window.cancelAnimationFrame(handle),
): LatestFrameQueue<T> {
  let frame: number | null = null
  let latest: T | undefined
  let pending = false

  const applyLatest = () => {
    if (!pending) return
    const value = latest as T
    latest = undefined
    pending = false
    apply(value)
  }

  return {
    cancel() {
      if (frame !== null) cancelFrame(frame)
      frame = null
      latest = undefined
      pending = false
    },
    flush() {
      if (frame !== null) cancelFrame(frame)
      frame = null
      applyLatest()
    },
    schedule(value) {
      latest = value
      pending = true
      if (frame !== null) return
      frame = requestFrame(() => {
        frame = null
        applyLatest()
      })
    },
  }
}

export const ALLOCATION_DRAG_TYPE = 'application/x-yard-allocation'
const ALLOCATION_DRAG_ATTRIBUTE = 'data-yard-allocation-dragging'

export interface AllocationDragPayload {
  kind: 'profile' | 'worker'
  id: string
  mode?: 'allocate' | 'handoff'
}

export function setAllocationDragData(
  dataTransfer: DataTransfer,
  payload: AllocationDragPayload,
) {
  // Changing descendant hit testing during dragstart can stall Chromium's
  // native drag loop before it reaches the drop target.
  const frame = window.requestAnimationFrame(() =>
    document.documentElement.setAttribute(ALLOCATION_DRAG_ATTRIBUTE, 'true'),
  )
  const clear = () => {
    window.cancelAnimationFrame(frame)
    window.removeEventListener('dragend', clear)
    window.removeEventListener('drop', clear)
    document.documentElement.removeAttribute(ALLOCATION_DRAG_ATTRIBUTE)
  }
  window.addEventListener('dragend', clear, { once: true })
  window.addEventListener('drop', clear, { once: true })
  dataTransfer.effectAllowed =
    payload.kind === 'worker' && payload.mode === 'handoff' ? 'move' : 'copy'
  dataTransfer.setData(ALLOCATION_DRAG_TYPE, JSON.stringify(payload))
}

export function getAllocationDragData(
  dataTransfer: DataTransfer,
): AllocationDragPayload | null {
  try {
    const payload = JSON.parse(
      dataTransfer.getData(ALLOCATION_DRAG_TYPE),
    ) as Partial<AllocationDragPayload>
    if (
      (payload.kind === 'profile' || payload.kind === 'worker') &&
      typeof payload.id === 'string' &&
      payload.id &&
      (payload.mode === undefined ||
        payload.mode === 'allocate' ||
        payload.mode === 'handoff')
    ) {
      return { kind: payload.kind, id: payload.id, mode: payload.mode }
    }
  } catch {
    // Ignore malformed or unrelated browser drag data.
  }
  return null
}

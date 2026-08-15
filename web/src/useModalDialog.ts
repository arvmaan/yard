import {
  useCallback,
  useEffect,
  useRef,
  type RefObject,
} from 'react'

const FOCUSABLE_SELECTOR =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [contenteditable="true"], [tabindex]:not([tabindex="-1"])'

interface ModalDialogOptions {
  active?: boolean
  canClose?: boolean
  dialogRef: RefObject<HTMLElement | null>
  initialFocusRef?: RefObject<HTMLElement | null>
  onClose: () => void
  returnFocus?: HTMLElement | null
}

interface InertSnapshot {
  ariaHidden: string | null
  element: HTMLElement
  inert: boolean
}

function focusableElements(dialog: HTMLElement) {
  return Array.from(
    dialog.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR),
  ).filter(
    (element) =>
      element.getAttribute('aria-hidden') !== 'true' &&
      element.getClientRects().length > 0,
  )
}

function makeOutsideContentInert(dialog: HTMLElement) {
  const snapshots: InertSnapshot[] = []
  let current: HTMLElement | null = dialog

  while (current?.parentElement) {
    const parentElement: HTMLElement = current.parentElement
    for (const sibling of parentElement.children) {
      if (sibling === current || !(sibling instanceof HTMLElement)) continue
      snapshots.push({
        ariaHidden: sibling.getAttribute('aria-hidden'),
        element: sibling,
        inert: sibling.inert,
      })
      sibling.inert = true
      sibling.setAttribute('aria-hidden', 'true')
    }
    current = parentElement
  }

  return () => {
    for (const snapshot of snapshots.reverse()) {
      snapshot.element.inert = snapshot.inert
      if (snapshot.ariaHidden === null) {
        snapshot.element.removeAttribute('aria-hidden')
      } else {
        snapshot.element.setAttribute('aria-hidden', snapshot.ariaHidden)
      }
    }
  }
}

export function useModalDialog({
  active = true,
  canClose = true,
  dialogRef,
  initialFocusRef,
  onClose,
  returnFocus = null,
}: ModalDialogOptions) {
  const canCloseRef = useRef(canClose)
  const closingRef = useRef(false)
  const onCloseRef = useRef(onClose)
  const openingFocusRef = useRef<HTMLElement | null>(
    document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null,
  )
  const returnFocusRef = useRef(returnFocus)
  canCloseRef.current = canClose
  onCloseRef.current = onClose
  returnFocusRef.current = returnFocus

  const requestClose = useCallback(() => {
    if (!canCloseRef.current || closingRef.current) return
    closingRef.current = true
    onCloseRef.current()
  }, [])

  useEffect(() => {
    if (!active || !dialogRef.current) return
    closingRef.current = false
    const dialog = dialogRef.current
    const focusTarget =
      returnFocusRef.current ??
      openingFocusRef.current
    const restoreOutsideContent = makeOutsideContentInert(dialog)
    const frame = window.requestAnimationFrame(() => {
      if (dialog.contains(document.activeElement)) return
      const initial = initialFocusRef?.current ?? focusableElements(dialog)[0]
      initial?.focus()
    })
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        if (!canCloseRef.current) return
        event.preventDefault()
        requestClose()
        return
      }
      if (event.key !== 'Tab') return

      const focusable = focusableElements(dialog)
      const first = focusable[0]
      const last = focusable.at(-1)
      if (!first || !last) {
        event.preventDefault()
        return
      }
      const focused = document.activeElement
      if (
        event.shiftKey &&
        (focused === first || !dialog.contains(focused))
      ) {
        event.preventDefault()
        last.focus()
      } else if (
        !event.shiftKey &&
        (focused === last || !dialog.contains(focused))
      ) {
        event.preventDefault()
        first.focus()
      }
    }

    window.addEventListener('keydown', handleKeyDown)
    return () => {
      window.cancelAnimationFrame(frame)
      window.removeEventListener('keydown', handleKeyDown)
      restoreOutsideContent()
      window.setTimeout(() => focusTarget?.focus(), 0)
    }
  }, [active, dialogRef, initialFocusRef, requestClose])

  return requestClose
}

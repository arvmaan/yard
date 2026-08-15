import { useId, useRef } from 'react'
import { SquareTerminal, X } from 'lucide-react'
import {
  TerminalSession,
  type TerminalTarget,
} from './TerminalSession'
import { useModalDialog } from './useModalDialog'

interface TerminalWorkspaceProps {
  label: string
  onClose: () => void
  returnFocus: HTMLElement | null
  target: TerminalTarget
  terminalId?: string
}

export function TerminalWorkspace({
  label,
  onClose,
  returnFocus,
  target,
  terminalId,
}: TerminalWorkspaceProps) {
  const titleId = useId()
  const dialogRef = useRef<HTMLElement>(null)
  useModalDialog({
    dialogRef,
    onClose,
    returnFocus,
  })

  return (
    <div className="modal-backdrop terminal-backdrop" role="presentation">
      <section
        aria-labelledby={titleId}
        aria-modal="true"
        className="terminal-workspace"
        ref={dialogRef}
        role="dialog"
      >
        <header className="terminal-workspace__header">
          <span className="terminal-workspace__icon">
            <SquareTerminal aria-hidden="true" size={20} />
          </span>
          <div>
            <p className="eyebrow">Live terminal</p>
            <h2 id={titleId}>{label}</h2>
          </div>
          {terminalId ? (
            <code title={terminalId}>{terminalId}</code>
          ) : null}
          <button
            aria-label="Close terminal"
            className="icon-button"
            onClick={onClose}
            title="Close terminal"
            type="button"
          >
            <X aria-hidden="true" size={18} />
          </button>
        </header>
        <TerminalSession target={target} />
      </section>
    </div>
  )
}

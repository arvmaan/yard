import { FitAddon } from '@xterm/addon-fit'
import { Terminal } from '@xterm/xterm'
import { useEffect, useRef, useState } from 'react'
import {
  assignmentTerminalWebSocketUrl,
  coordinationNodeTerminalWebSocketUrl,
  orchestratorTerminalWebSocketUrl,
  yardOrchestratorTerminalWebSocketUrl,
} from './api'
import {
  readTheme,
  terminalTheme,
  THEME_CHANGE_EVENT,
  type YardTheme,
} from './theme'
import type {
  TerminalClientMessage,
  TerminalFrameMessage,
  TerminalServerMessage,
} from './types'
import '@xterm/xterm/css/xterm.css'

type ConnectionState =
  | { kind: 'connecting'; detail: string }
  | { kind: 'connected'; detail: string }
  | { kind: 'closed'; detail: string }
  | { kind: 'error'; detail: string }

interface FrameMetadata {
  seq: number
  width: number
  height: number
  full: boolean
}

export type TerminalTarget =
  | {
      kind: 'assignment'
      projectId: string
      assignmentId: string
    }
  | {
      kind: 'orchestrator'
      projectId: string
      workerId: string
    }
  | {
      kind: 'yard-orchestrator'
      workerId: string
    }
  | {
      kind: 'coordination-node'
      nodeId: string
      workerId: string
    }

function decodeBase64(value: string): Uint8Array {
  const decoded = atob(value)
  return Uint8Array.from(decoded, (character) => character.charCodeAt(0))
}

function isTerminalFrameMessage(
  message: TerminalServerMessage,
): message is TerminalFrameMessage {
  return (
    message.type === 'terminal.frame' &&
    typeof message.bytes === 'string' &&
    typeof message.seq === 'number' &&
    typeof message.width === 'number' &&
    typeof message.height === 'number' &&
    typeof message.full === 'boolean'
  )
}

function sendMessage(
  socket: WebSocket,
  message: TerminalClientMessage,
): boolean {
  if (socket.readyState !== WebSocket.OPEN) return false
  socket.send(JSON.stringify(message))
  return true
}

export function TerminalSession({ target }: { target: TerminalTarget }) {
  const hostRef = useRef<HTMLDivElement>(null)
  const [theme, setTheme] = useState<YardTheme>(readTheme)
  const [connection, setConnection] = useState<ConnectionState>({
    kind: 'connecting',
    detail: 'Connecting',
  })
  const [frame, setFrame] = useState<FrameMetadata | null>(null)
  const assignmentId =
    target.kind === 'assignment' ? target.assignmentId : null
  const workerId = target.kind === 'orchestrator' ? target.workerId : null
  const projectId =
    target.kind === 'assignment' || target.kind === 'orchestrator'
      ? target.projectId
      : null
  const nodeId = target.kind === 'coordination-node' ? target.nodeId : null
  const targetKind = target.kind

  useEffect(() => {
    const host = hostRef.current
    if (!host) return

    let disposed = false
    let socket: WebSocket | null = null
    let released = false
    let fitAnimationFrame = 0
    let connectAnimationFrame = 0
    let lastSequence = -1
    let lastSentSize = ''
    let receivedInitialFrame = false

    setConnection({ kind: 'connecting', detail: 'Connecting' })
    setFrame(null)

    const terminal = new Terminal({
      convertEol: false,
      cursorBlink: true,
      cursorInactiveStyle: 'outline',
      cursorStyle: 'block',
      fontFamily: '"IBM Plex Mono", monospace',
      fontSize: 11,
      letterSpacing: 0,
      lineHeight: 1.2,
      minimumContrastRatio: 4.5,
      screenReaderMode: true,
      scrollback: 10000,
      smoothScrollDuration: 0,
      theme: terminalTheme(readTheme()),
    })
    const fitAddon = new FitAddon()
    terminal.loadAddon(fitAddon)
    terminal.open(host)
    terminal.focus()
    const handleThemeChange = (event: Event) => {
      const nextTheme = (event as CustomEvent<YardTheme>).detail
      terminal.options.theme = terminalTheme(nextTheme)
      setTheme(nextTheme)
      window.requestAnimationFrame(() => {
        if (!disposed && terminal.rows > 0) {
          terminal.refresh(0, terminal.rows - 1)
        }
      })
    }
    window.addEventListener(THEME_CHANGE_EVENT, handleThemeChange)

    const sendResize = (force = false) => {
      if (!socket) return
      const size = `${terminal.cols}x${terminal.rows}`
      if (!force && size === lastSentSize) return
      if (
        sendMessage(socket, {
          type: 'terminal.resize',
          cols: terminal.cols,
          rows: terminal.rows,
        })
      ) {
        lastSentSize = size
      }
    }

    const fit = () => {
      fitAnimationFrame = 0
      if (disposed || !host.isConnected) return
      const dimensions = fitAddon.proposeDimensions()
      if (
        dimensions &&
        (dimensions.cols !== terminal.cols ||
          dimensions.rows !== terminal.rows)
      ) {
        fitAddon.fit()
      }
      sendResize()
    }

    const scheduleFit = () => {
      if (!fitAnimationFrame) {
        fitAnimationFrame = window.requestAnimationFrame(fit)
      }
    }

    const resizeObserver = new ResizeObserver(scheduleFit)
    resizeObserver.observe(host)

    const inputSubscription = terminal.onData((text) => {
      if (socket) {
        sendMessage(socket, { type: 'terminal.input', text })
      }
    })
    const handleWheel = (event: WheelEvent) => {
      const lineDelta =
        Math.sign(event.deltaY) *
        Math.max(1, Math.round(Math.abs(event.deltaY) / 40))
      if (!lineDelta) return
      terminal.scrollLines(lineDelta)
      event.preventDefault()
      event.stopPropagation()
    }
    host.addEventListener('wheel', handleWheel, {
      capture: true,
      passive: false,
    })

    connectAnimationFrame = window.requestAnimationFrame(() => {
      connectAnimationFrame = 0
      if (disposed) return

      fitAddon.fit()
      const url = targetKind === 'yard-orchestrator'
        ? yardOrchestratorTerminalWebSocketUrl(
            terminal.cols,
            terminal.rows,
          )
        : targetKind === 'coordination-node' && nodeId
          ? coordinationNodeTerminalWebSocketUrl(
              nodeId,
              terminal.cols,
              terminal.rows,
            )
        : targetKind === 'assignment' && assignmentId && projectId
          ? assignmentTerminalWebSocketUrl(
              projectId,
              assignmentId,
              terminal.cols,
              terminal.rows,
            )
          : orchestratorTerminalWebSocketUrl(
              projectId ?? '',
              terminal.cols,
              terminal.rows,
            )
      socket = new WebSocket(url)

      socket.onopen = () => {
        if (disposed) return
        setConnection({ kind: 'connected', detail: 'Connected' })
        sendResize(true)
      }

      socket.onmessage = (event) => {
        if (disposed || typeof event.data !== 'string') return

        let message: TerminalServerMessage
        try {
          message = JSON.parse(event.data) as TerminalServerMessage
        } catch {
          setConnection({
            kind: 'error',
            detail: 'Terminal protocol error',
          })
          socket?.close(1000, 'Protocol error')
          return
        }

        if (isTerminalFrameMessage(message)) {
          if (message.seq <= lastSequence) return
          lastSequence = message.seq
          try {
            // Reset only for the first authoritative snapshot. Later full
            // frames may follow resize/reconnect state and must not erase
            // bytes already accepted for this lease.
            if (message.full && !receivedInitialFrame) terminal.reset()
            terminal.write(decodeBase64(message.bytes))
            receivedInitialFrame = true
            setFrame({
              seq: message.seq,
              width: message.width,
              height: message.height,
              full: message.full,
            })
          } catch {
            setConnection({
              kind: 'error',
              detail: 'Terminal frame could not be decoded',
            })
            socket?.close(1000, 'Invalid terminal frame')
          }
          return
        }

        if (message.type === 'terminal.closed') {
          released = true
          setConnection({
            kind: 'closed',
            detail: message.reason ?? message.code ?? 'Session closed',
          })
          socket?.close(1000)
        }
      }

      socket.onerror = () => {
        if (!disposed) {
          setConnection({
            kind: 'error',
            detail: 'Terminal connection unavailable',
          })
        }
      }

      socket.onclose = (event) => {
        if (disposed) return
        setConnection((current) => {
          if (current.kind === 'closed' || current.kind === 'error') {
            return current
          }
          return {
            kind: 'closed',
            detail: event.reason || 'Terminal connection closed',
          }
        })
      }
    })

    return () => {
      disposed = true
      resizeObserver.disconnect()
      inputSubscription.dispose()
      window.removeEventListener(THEME_CHANGE_EVENT, handleThemeChange)
      host.removeEventListener('wheel', handleWheel, true)
      if (fitAnimationFrame) {
        window.cancelAnimationFrame(fitAnimationFrame)
      }
      if (connectAnimationFrame) {
        window.cancelAnimationFrame(connectAnimationFrame)
      }
      if (socket) {
        socket.onclose = null
        socket.onerror = null
        socket.onmessage = null
        socket.onopen = null
        if (!released) {
          released = sendMessage(socket, { type: 'terminal.release' })
        }
        socket.close(1000)
      }
      terminal.dispose()
    }
  }, [assignmentId, nodeId, projectId, targetKind, workerId])

  return (
    <div
      className="terminal-session"
      data-frame-full={frame?.full}
      data-frame-height={frame?.height}
      data-frame-sequence={frame?.seq}
      data-frame-width={frame?.width}
      data-state={connection.kind}
      data-terminal-theme={theme}
    >
      <div
        aria-label={
          targetKind === 'orchestrator' ||
          targetKind === 'coordination-node'
            ? 'Interactive orchestrator terminal'
            : 'Interactive terminal'
        }
        className="terminal-session__viewport"
        ref={hostRef}
        role="application"
      />
      <div
        aria-live="polite"
        className="terminal-session__status"
        role="status"
      >
        <span aria-hidden="true" />
        {connection.detail}
      </div>
    </div>
  )
}

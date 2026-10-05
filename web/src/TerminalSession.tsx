import { FitAddon } from '@xterm/addon-fit'
import { Terminal } from '@xterm/xterm'
import { ArrowDownToLine, RefreshCw } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import {
  assignmentTerminalWebSocketUrl,
  coordinationNodeTerminalWebSocketUrl,
  fetchAssignmentTerminalOutput,
  fetchCoordinationNodeTerminalOutput,
  fetchOrchestratorTerminalOutput,
  fetchYardOrchestratorTerminalOutput,
  orchestratorTerminalWebSocketUrl,
  type TerminalOutputFormat,
  yardOrchestratorTerminalWebSocketUrl,
} from './api'
import {
  HISTORY_EXTEND_RETRY_MS,
  HISTORY_MAX_LINES,
  HISTORY_PROBE_BACKOFF_MS,
  HISTORY_REFRESH_INTERVAL_MS,
  HISTORY_ROW_LINES,
  HISTORY_SCROLLBACK_ROWS,
  type HistoryRead,
  historyEntryLines,
  historyModeEligible,
  historyRows,
  locateRows,
  MAX_HISTORY_PROBE_BACKOFF_MS,
  mergeScreen,
  olderHistorySeam,
  olderLinesStart,
  prependSeam,
  refreshSeam,
  rowKey,
  screenSeam,
  serializeRow,
  stripTerminalEscapes,
} from './terminalHistory'
import { TerminalHistoryView } from './terminalHistoryView'
import { chunkTerminalInput } from './terminalInput'
import {
  addPageScroll,
  addWheelSteps,
  answerScrollBatch,
  bracketedTerminalPaste,
  clampTerminalSize,
  clearScrollGate,
  createScrollGate,
  expireScrollBatch,
  formatTerminalBytes,
  pageScrollMessage,
  pointerTerminalCell,
  readTerminalScreenReaderPreference,
  pageScrollLines,
  SCROLL_BATCH_TIMEOUT_MS,
  scrollBatchMessages,
  takeScrollBatch,
  type TerminalCell,
  type TerminalSize,
  WHEEL_LINES_PER_SCROLL,
  wheelEventSteps,
  wrapHerdrFrame,
} from './terminalProtocol'
import {
  readTheme,
  terminalTheme,
  THEME_CHANGE_EVENT,
  type ThemeId,
} from './theme'
import type {
  TerminalClientMessage,
  TerminalFrameMessage,
  TerminalScrollMessage,
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

// Herdr frames repaint a fixed screen with cursor positioning and never feed
// lines into xterm's buffer, so a local scrollback could only ever hold stale
// text (such as an old prompt and status bar above a newer answer). Earlier
// output is reached by asking Herdr to scroll with `terminal.scroll`.
const TERMINAL_SCROLLBACK_LINES = 0
const TERMINAL_RECONNECT_BASE_DELAY_MS = 1_000
const TERMINAL_RECONNECT_MAX_DELAY_MS = 8_000
// Trailing debounce for PTY resizes: each one is a SIGWINCH and a full TUI
// redraw in the agent, so layout churn must not reach Herdr frame by frame.
const TERMINAL_RESIZE_DEBOUNCE_MS = 150

function writeTerminal(
  terminal: Terminal,
  data: string | Uint8Array,
): Promise<void> {
  return new Promise((resolve) => terminal.write(data, resolve))
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
    typeof message.full === 'boolean' &&
    (message.scrolls === undefined || typeof message.scrolls === 'number')
  )
}

// xterm adds the characters it collected during its ~1 s announce debounce
// to the live region in one append, after the frame writes that empty it,
// so at 60 frames per second one append held many screens and its layout
// ran past 50 ms. Keep only the latest `limit()` characters.
function capLiveRegion(host: HTMLElement, limit: () => number) {
  const liveRegion = host.querySelector('.live-region')
  const text = Object.getOwnPropertyDescriptor(Node.prototype, 'textContent')
  if (!liveRegion || !text?.get || !text.set) return
  const { get, set } = text
  Object.defineProperty(liveRegion, 'textContent', {
    configurable: true,
    get() {
      return get.call(liveRegion)
    },
    set(value: string | null) {
      const max = limit()
      set.call(
        liveRegion,
        value && value.length > max ? value.slice(-max) : value,
      )
    },
  })
}

function readScreenReaderPreference() {
  try {
    return readTerminalScreenReaderPreference(window.localStorage)
  } catch {
    return false
  }
}

// One row of local history: its comparison key, plain text (for widths),
// and ANSI text (to rebuild the view).
interface HistoryRecord {
  key: string
  text: string
  ansi: string
}

type HistoryMode = 'live' | 'history'
// Whether history still ends with Herdr's latest output: 'updating' while a
// fresh read is joined on, 'behind' when newer output could not be joined.
type HistoryTail = 'current' | 'updating' | 'behind'

function sendMessage(
  socket: WebSocket,
  message: TerminalClientMessage,
): boolean {
  if (socket.readyState !== WebSocket.OPEN) return false
  socket.send(JSON.stringify(message))
  return true
}

function applyResolvedTerminalTheme(
  terminal: Terminal,
  theme: ThemeId,
) {
  // xterm permits OSC color changes from the byte stream. Use a fresh object
  // so its option service reapplies Yard's selected theme after each frame.
  terminal.options.theme = terminalTheme(theme)
}

function protectResolvedTerminalTheme(terminal: Terminal) {
  const indexedQuery = /^\d+;\?(?:;\d+;\?)*$/
  return [
    terminal.parser.registerOscHandler(
      4,
      (data) => !indexedQuery.test(data),
    ),
    ...[10, 11, 12].map((identifier) =>
      terminal.parser.registerOscHandler(
        identifier,
        (data) => data !== '?',
      ),
    ),
    ...[104, 110, 111, 112].map((identifier) =>
      terminal.parser.registerOscHandler(identifier, () => true),
    ),
  ]
}

export function TerminalSession({ target }: { target: TerminalTarget }) {
  const hostRef = useRef<HTMLDivElement>(null)
  const terminalRef = useRef<Terminal | null>(null)
  const reconnectAttemptRef = useRef(0)
  const [theme, setTheme] = useState<ThemeId>(readTheme)
  const [connection, setConnection] = useState<ConnectionState>({
    kind: 'connecting',
    detail: 'Connecting',
  })
  const [connectionAttempt, setConnectionAttempt] = useState(0)
  const [frame, setFrame] = useState<FrameMetadata | null>(null)
  const [terminalSize, setTerminalSize] = useState<TerminalSize | null>(null)
  // Lines in xterm's active buffer. With no local scrollback this is always
  // the screen height, so it shows that nothing piled up locally.
  const [bufferLines, setBufferLines] = useState<number | null>(null)
  const [inputNotice, setInputNotice] = useState<string | null>(null)
  // Herdr keeps one scroll offset per pane, so the view stays scrolled back
  // until input or a scroll reset. Show that, with a way back to the latest.
  const [scrolledBack, setScrolledBack] = useState(false)
  // Local history mode: a read-only xterm over the live one.
  const [historyMode, setHistoryMode] = useState<HistoryMode>('live')
  const [historyNote, setHistoryNote] = useState<string | null>(null)
  const [historyTail, setHistoryTail] = useState<HistoryTail>('current')
  const historyViewRef = useRef<TerminalHistoryView | null>(null)
  const jumpToLatestRef = useRef<(() => void) | null>(null)
  const assignmentId =
    target.kind === 'assignment' ? target.assignmentId : null
  const workerId = 'workerId' in target ? target.workerId : null
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
    let reconnectScheduled = false
    let reconnectTimer = 0
    let fitAnimationFrame = 0
    let settleFitTimer = 0
    let connectAnimationFrame = 0
    let frameAnimationFrame = 0
    let resizeSendTimer = 0
    // Scroll flow control: at most one batch waits for Herdr's repaint.
    let scrollGate = createScrollGate()
    let scrollTimeoutTimer = 0
    // `terminal.scroll` messages sent on this socket; Yard tags each frame
    // with how many of them it had forwarded when it read the frame.
    let scrollMessagesSent = 0
    let lastWheelPoint: { x: number; y: number } | null = null
    let pendingFrame: FrameMetadata | null = null
    let lastSequence = -1
    let lastSentSize = ''
    let frameWriteQueue = Promise.resolve()
    // Net lines this view has asked Herdr to scroll up since the last input
    // or reset. An approximation for mouse-aware apps (one report per
    // command), exact for Herdr's own scrollback.
    let scrolledUpLines = 0
    // Signed lines (negative = up) of the scroll batch Herdr has not answered.
    let inFlightScrollLines = 0
    // Local history mode (terminalHistory.ts). `historyRecords` describe the
    // last `historyRecords.length` rows of the history view's buffer.
    let history: TerminalHistoryView | null = null
    let historyRecords: HistoryRecord[] = []
    let historyTruncated = false
    let historyExtended = false
    let historyExtending: AbortController | null = null
    // An older-lines read that found nothing older yet may be retried, but
    // not before this time.
    let historyExtendAt = 0
    let historyRefreshing: AbortController | null = null
    // The rest of Herdr's coloured rows, read right after history opened.
    let historyFilling: AbortController | null = null
    let historyRefreshAt = 0
    // Set when newer output can no longer be joined to this history.
    let historyGap = false
    let historyProbe: AbortController | null = null
    // Herdr's scrollback depth at the last probe: 0 means a full-screen app
    // (no scrollback), so the next probe waits for Herdr's frame first.
    let historyProbeMaxOffset: number | null = null
    let historyProbeAfterFrame = false
    // Set from an eligible probe's answer until its view is shown or
    // dropped: scroll batches sent meanwhile must not start another probe
    // (its generation bump would discard the view being built).
    let historyEntering = false
    // The grid of the last frame Herdr sent. Right after a resize Herdr can
    // still paint (and read) rows laid out for the old grid.
    let lastFrameWidth = 0
    let lastFrameHeight = 0
    // Bumped whenever history mode is left or a probe is dropped, so late
    // reads and view builds are discarded.
    let historyGeneration = 0
    let historyProbeAt = 0
    let historyProbeBackoff = HISTORY_PROBE_BACKOFF_MS
    let historyMergeFrame = 0
    let historyMergeQueued = false
    // Serializes changes to the history view (merges, the older-lines
    // rebuild) so each one sees the rows the previous one wrote.
    let historyLock: Promise<void> = Promise.resolve()

    setConnection({ kind: 'connecting', detail: 'Connecting' })
    setFrame(null)
    setBufferLines(null)
    setInputNotice(null)
    setScrolledBack(false)
    setHistoryMode('live')
    setHistoryNote(null)
    setHistoryTail('current')

    const trackScroll = (direction: 'up' | 'down', lines: number) => {
      scrolledUpLines = Math.max(
        0,
        scrolledUpLines + (direction === 'up' ? lines : -lines),
      )
      if (!disposed) setScrolledBack(scrolledUpLines > 0)
    }

    // Any input makes Herdr return the pane to its latest output.
    const clearScrolledBack = () => {
      scrolledUpLines = 0
      if (!disposed) setScrolledBack(false)
    }

    const sendScroll = (message: TerminalScrollMessage) => {
      if (!socket || !sendMessage(socket, message)) return false
      scrollMessagesSent += 1
      trackScroll(message.direction, message.lines * (message.count ?? 1))
      return true
    }

    const scheduleReconnect = () => {
      if (disposed || released || reconnectScheduled) return
      reconnectScheduled = true
      reconnectAttemptRef.current += 1
      const delay = Math.min(
        TERMINAL_RECONNECT_MAX_DELAY_MS,
        TERMINAL_RECONNECT_BASE_DELAY_MS *
          2 ** Math.min(reconnectAttemptRef.current - 1, 3),
      )
      setConnection({
        kind: 'error',
        detail: `Terminal connection unavailable · retrying in ${Math.ceil(delay / 1_000)}s`,
      })
      reconnectTimer = window.setTimeout(() => {
        reconnectTimer = 0
        if (!disposed) {
          setConnectionAttempt((attempt) => attempt + 1)
        }
      }, delay)
    }

    // Off unless the user opted in. When on, the live region is emptied after
    // every frame and capped at one screen so Herdr's repaints cannot pile up
    // in it.
    const screenReaderMode = readScreenReaderPreference()
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
      screenReaderMode,
      scrollback: TERMINAL_SCROLLBACK_LINES,
      smoothScrollDuration: 0,
      theme: terminalTheme(readTheme()),
    })
    terminalRef.current = terminal
    const terminalThemeHandlers = protectResolvedTerminalTheme(terminal)
    const fitAddon = new FitAddon()
    terminal.loadAddon(fitAddon)
    terminal.open(host)
    terminal.focus()
    // The size Herdr should render: the fitted size clamped to the server's
    // accepted range. Resize messages always send this, never a transient
    // xterm size such as the renderer nudge below.
    let targetSize = clampTerminalSize({
      cols: terminal.cols,
      rows: terminal.rows,
    })
    setTerminalSize({ cols: terminal.cols, rows: terminal.rows })
    if (screenReaderMode) {
      capLiveRegion(host, () => targetSize.cols * targetSize.rows)
    }
    const resizeSubscription = terminal.onResize(({ cols, rows }) => {
      if (disposed) return
      setTerminalSize({ cols, rows })
      setBufferLines(terminal.buffer.active.length)
    })
    const handleThemeChange = (event: Event) => {
      setTheme((event as CustomEvent<ThemeId>).detail)
    }
    window.addEventListener(THEME_CHANGE_EVENT, handleThemeChange)

    const sendResize = (force = false) => {
      if (!socket) return
      const size = `${targetSize.cols}x${targetSize.rows}`
      if (!force && size === lastSentSize) return
      if (
        sendMessage(socket, {
          type: 'terminal.resize',
          cols: targetSize.cols,
          rows: targetSize.rows,
        })
      ) {
        lastSentSize = size
      }
    }

    const scheduleResizeSend = () => {
      if (resizeSendTimer) window.clearTimeout(resizeSendTimer)
      resizeSendTimer = window.setTimeout(() => {
        resizeSendTimer = 0
        if (!disposed) sendResize()
      }, TERMINAL_RESIZE_DEBOUNCE_MS)
    }

    // Size xterm to its host, clamped to the range Herdr accepts. When the
    // host is larger than the clamp, the extra space stays as padding so the
    // local grid always matches the grid Herdr renders.
    const fitTerminal = () => {
      const proposed = fitAddon.proposeDimensions()
      if (
        !proposed ||
        !Number.isFinite(proposed.cols) ||
        !Number.isFinite(proposed.rows)
      ) {
        return
      }
      const size = clampTerminalSize(proposed)
      if (size.cols !== targetSize.cols || size.rows !== targetSize.rows) {
        // History rows are laid out for the old grid.
        dropHistoryProbe()
        exitHistory()
      }
      if (size.cols !== terminal.cols || size.rows !== terminal.rows) {
        if (size.cols === proposed.cols && size.rows === proposed.rows) {
        fitAddon.fit()
        } else {
          terminal.resize(size.cols, size.rows)
        }
      }
      targetSize = size
      }

    const fit = () => {
      fitAnimationFrame = 0
      if (disposed || !host.isConnected) return
      fitTerminal()
      scheduleResizeSend()
    }

    // xterm.js only recomputes its renderer-level pixel dimensions (the
    // scrollbar included) when terminal.resize() is called with a value
    // that actually differs from the current one — calling fit()/resize()
    // again with the same already-correct cols/rows is a no-op. Resizing
    // away by one column and back forces that recomputation deliberately,
    // the same way an incidental browser-window resize does. It is local
    // only: resize messages send targetSize, captured before the nudge.
    // The nudge widens rather than narrows: Herdr writes every cell of every
    // row, so narrowing would make xterm reflow each full row onto two lines
    // and, with no scrollback, drop the screen Herdr will not resend.
    const nudgeRendererResync = () => {
      const cols = terminal.cols
      const rows = terminal.rows
      terminal.resize(cols + 1, rows)
      window.requestAnimationFrame(() => {
        if (disposed) return
        terminal.resize(cols, rows)
      })
    }

    const scheduleFit = () => {
      if (!fitAnimationFrame) {
        fitAnimationFrame = window.requestAnimationFrame(fit)
      }
    }

    const resizeObserver = new ResizeObserver(scheduleFit)
    resizeObserver.observe(host)

    const readHistory = (
      lines: number,
      format: TerminalOutputFormat,
      signal: AbortSignal,
    ): Promise<HistoryRead> => {
      if (targetKind === 'yard-orchestrator') {
        return fetchYardOrchestratorTerminalOutput(lines, signal, format)
      }
      if (targetKind === 'coordination-node' && nodeId) {
        return fetchCoordinationNodeTerminalOutput(nodeId, lines, signal, format)
      }
      if (targetKind === 'assignment' && projectId && assignmentId) {
        return fetchAssignmentTerminalOutput(
          projectId,
          assignmentId,
          lines,
          signal,
          format,
        )
      }
      return fetchOrchestratorTerminalOutput(
        projectId ?? '',
        lines,
        signal,
        format,
      )
    }

    // The live screen's rows, as Herdr last painted them.
    const liveRowKeys = () => {
      const buffer = terminal.buffer.active
      const keys: string[] = []
      for (let row = 0; row < terminal.rows; row += 1) {
        const line = buffer.getLine(buffer.viewportY + row)
        keys.push(line ? rowKey(line.translateToString(true)) : '')
      }
      return keys
    }

    const liveRecords = (keys: readonly string[]): HistoryRecord[] => {
      const buffer = terminal.buffer.active
      const cell = buffer.getNullCell()
      return keys.map((key, row) => {
        const line = buffer.getLine(buffer.viewportY + row)
        return {
          key,
          text: line?.translateToString(true) ?? '',
          ansi: line ? serializeRow(line, terminal.cols, cell) : '',
        }
      })
    }

    // Read through a call so TypeScript does not narrow it across awaits.
    const isHistoryView = (view: TerminalHistoryView) => history === view

    const historyBufferRow = (view: TerminalHistoryView, index: number) =>
      view.bufferRows() - historyRecords.length + index

    const exitHistory = () => {
      historyGeneration += 1
      // Reads still in flight finish and are discarded rather than aborted:
      // an aborted fetch closes its HTTP connection, and a probe on a new
      // connection costs the next scroll-up a round trip (the connection
      // setup reaches the browser ahead of Herdr's frame, which then no
      // longer fits the tunnel's first congestion window).
      historyExtending = null
      historyRefreshing = null
      historyFilling = null
      historyGap = false
      if (historyMergeFrame) window.cancelAnimationFrame(historyMergeFrame)
      historyMergeFrame = 0
      if (!history) return false
      history.dispose()
      history = null
      historyViewRef.current = null
      historyRecords = []
      if (!disposed) {
        setHistoryMode('live')
        setHistoryNote(null)
        setHistoryTail('current')
        terminal.focus()
      }
      return true
    }

    const dropHistoryProbe = () => {
      if (!historyProbe) return
      // Discarded, not aborted (see exitHistory).
      historyProbe = null
      historyGeneration += 1
    }

    // Input changes what the pane shows, so the next scroll probes again.
    const resetHistoryProbe = () => {
      dropHistoryProbe()
      historyProbeAfterFrame = false
      historyProbeAt = 0
      historyProbeBackoff = HISTORY_PROBE_BACKOFF_MS
    }

    const backOffHistoryProbe = () => {
      historyProbeAt = performance.now() + historyProbeBackoff
      historyProbeBackoff = Math.min(
        MAX_HISTORY_PROBE_BACKOFF_MS,
        historyProbeBackoff * 2,
      )
    }

    // Scroll input Herdr has not painted yet: the batch in flight and what
    // is still queued. History mode applies it locally at the handoff.
    const unshownScrollLines = () =>
      (scrollGate.inFlight ? inFlightScrollLines : 0) +
      Math.trunc(scrollGate.wheel) * WHEEL_LINES_PER_SCROLL +
      scrollGate.pages * pageScrollLines(targetSize.rows)

    const withHistoryLock = (change: () => Promise<void>) => {
      const run = historyLock.then(change)
      historyLock = run.catch(() => undefined)
      return run
    }

    // End the history with Herdr's latest screen once the live terminal
    // shows it, replacing the rows it overlaps (see screenSeam).
    const mergeLiveScreen = () => {
      if (!history || historyMergeQueued) return historyLock
      historyMergeQueued = true
      return withHistoryLock(async () => {
        historyMergeQueued = false
        const view = history
        if (!view) return
        const keys = liveRowKeys()
        const historyKeys = historyRecords.map((record) => record.key)
        const seam = screenSeam(historyKeys, keys)
        if (seam === null) {
          // More than a screen of output arrived since history last joined
          // the live screen: nothing on screen is in history any more.
          if (
            keys.some(Boolean) &&
            locateRows(historyKeys, keys, historyKeys.length) === null
          ) {
            refreshHistoryTail()
          }
          return
        }
        if (
          seam + keys.length === historyKeys.length &&
          keys.every((key, row) => historyKeys[seam + row] === key)
        ) {
          return
        }
        const records = liveRecords(keys)
        const merged = await view.replaceTail(
          historyBufferRow(view, seam),
          records.map((record) => record.ansi),
        )
        if (merged && isHistoryView(view)) {
          historyRecords = mergeScreen(historyRecords, seam, records)
        }
      })
    }

    // Join a fresh read of Herdr's latest rows onto the history when the
    // live screen alone no longer joins it (see refreshSeam). The live
    // screen then joins the refreshed tail as usual.
    const refreshHistoryTail = () => {
      if (!history || historyRefreshing || historyGap) return
      const now = performance.now()
      if (now < historyRefreshAt) return
      historyRefreshAt = now + HISTORY_REFRESH_INTERVAL_MS
      const view = history
      const generation = historyGeneration
      const controller = new AbortController()
      historyRefreshing = controller
      setHistoryTail('updating')
      readHistory(HISTORY_ROW_LINES, 'ansi', controller.signal)
        .then((read) =>
          withHistoryLock(async () => {
            if (historyRefreshing !== controller) return
            historyRefreshing = null
            if (generation !== historyGeneration) return
            if (!isHistoryView(view)) {
              // The older-lines rebuild replaced the view; the next live
              // frame asks again if it still does not join.
              historyRefreshAt = 0
              setHistoryTail('current')
              return
            }
            const records = historyRows(read.text).map((row) => {
              const text = stripTerminalEscapes(row)
              return { key: rowKey(text), text, ansi: row }
            })
            const join = refreshSeam(
              historyRecords.map((record) => record.key),
              records.map((record) => record.key),
              view.rows,
            )
            let joined = join !== null
            // Nothing past the join is new: the live screen merge tidies
            // the tail.
            if (join && join.from < records.length) {
              const tail = records.slice(join.from)
              joined = await view.replaceTail(
                historyBufferRow(view, join.seam),
                tail.map((record) => record.ansi),
              )
              if (generation !== historyGeneration) return
              if (!isHistoryView(view)) {
                historyRefreshAt = 0
                setHistoryTail('current')
                return
              }
              if (joined) historyRecords = mergeScreen(historyRecords, join.seam, tail)
            }
            if (!joined) {
              historyGap = true
              setHistoryTail('behind')
              return
            }
            setHistoryTail('current')
            scheduleHistoryMerge()
          }),
        )
        .catch(() => {
          if (historyRefreshing !== controller) return
          historyRefreshing = null
          setHistoryTail('behind')
        })
    }

    const scheduleHistoryMerge = () => {
      if (!history || historyMergeFrame) return
      historyMergeFrame = window.requestAnimationFrame(() => {
        historyMergeFrame = 0
        void mergeLiveScreen()
      })
    }

    const newHistoryView = () =>
      new TerminalHistoryView({
        host,
        cols: targetSize.cols,
        rows: targetSize.rows,
        theme: { ...terminal.options.theme },
        onPastBottom: () => {
          exitHistory()
        },
        onTop: () => extendHistory(),
      })

    // Make a fully built view the history view. It shows once positioned;
    // keyboard focus stays on the live terminal, so typing still goes to
    // the pane as it always does.
    const adoptHistoryView = (view: TerminalHistoryView) => {
      if (history !== view) history?.dispose()
      history = view
      historyViewRef.current = view
    }

    const showHistoryView = (view: TerminalHistoryView) => {
      adoptHistoryView(view)
      view.show()
      if (!disposed) setHistoryMode('history')
    }

    const enterHistory = async (read: HistoryRead, generation: number) => {
      const rows = historyRows(read.text)
      if (rows.length === 0) return
      const view = newHistoryView()
      await view.load(rows.map((text) => ({ text })))
      await view.settle()
      if (disposed || generation !== historyGeneration || history) {
        view.dispose()
        return
      }
      historyRecords = rows.map((row) => {
        const text = stripTerminalEscapes(row)
        return { key: rowKey(text), text, ansi: row }
      })
      historyTruncated = read.truncated
      historyExtended = false
      historyExtendAt = 0
      historyRefreshAt = 0
      historyGap = false
      adoptHistoryView(view)
      // If the live terminal already shows Herdr's latest screen, end the
      // history with it; otherwise that happens once Herdr repaints below.
      await mergeLiveScreen()
      if (disposed || !isHistoryView(view)) return
      // Open on exactly the rows the live terminal shows now, then carry on
      // with scroll input Herdr has not painted yet.
      const keys = historyRecords.map((record) => record.key)
      const expectedTop =
        keys.length - targetSize.rows - (read.scroll?.offset_from_bottom ?? 0)
      const top = locateRows(keys, liveRowKeys(), expectedTop) ?? expectedTop
      view.scrollToRow(historyBufferRow(view, Math.max(0, top)))
      view.scrollLines(unshownScrollLines())
      showHistoryView(view)
      // Herdr's own view returns to the latest output underneath.
      resetScrollGate()
      if (socket) sendMessage(socket, { type: 'terminal.scroll_reset' })
      clearScrolledBack()
      if (read.truncated) fillHistory()
    }

    // History opened on the entry read's few screens. Read the rest of
    // Herdr's coloured rows and put the older ones above, keeping the rows
    // in view where they are.
    const fillHistory = () => {
      if (!history || historyFilling) return
      const current = history
      const generation = historyGeneration
      const controller = new AbortController()
      historyFilling = controller
      readHistory(HISTORY_ROW_LINES, 'ansi', controller.signal)
        .then((read) =>
          withHistoryLock(async () => {
            if (historyFilling !== controller) return
            historyFilling = null
            if (generation !== historyGeneration || !isHistoryView(current)) return
            const older = historyRows(read.text).map((row) => {
              const text = stripTerminalEscapes(row)
              return { key: rowKey(text), text, ansi: row }
            })
            const top = prependSeam(
              older.map((record) => record.key),
              historyRecords.map((record) => record.key),
              current.rows,
            )
            // Not found (or found twice): the older lines at the top join
            // by content instead (extendHistory).
            if (top === null || top === 0) {
              if (top === 0) historyTruncated = read.truncated
              if (current.atTop()) extendHistory()
              return
            }
            const records = [...older.slice(0, top), ...historyRecords]
            const view = newHistoryView()
            await view.load(records.map((record) => ({ text: record.ansi })))
            await view.settle()
            if (
              disposed ||
              generation !== historyGeneration ||
              !isHistoryView(current)
            ) {
              view.dispose()
              return
            }
            const topIndex =
              current.topRow() - (current.bufferRows() - historyRecords.length)
            historyRecords = records
            historyTruncated = read.truncated
            view.scrollToRow(historyBufferRow(view, top + topIndex))
            showHistoryView(view)
            if (view.atTop()) extendHistory()
          }),
        )
        .catch(() => {
          if (historyFilling !== controller) return
          historyFilling = null
          if (history?.atTop()) extendHistory()
        })
    }

    // Probe once per gesture, right after a scroll-up reached Herdr: the
    // read carries Herdr's scroll position, which says whether Herdr
    // scrolled its own history (history mode) or passed the wheel to the
    // app (stay native).
    const startHistoryProbe = () => {
      if (history || historyProbe || historyEntering || disposed) return
      // The history view is visual only; screen-reader users keep Herdr's
      // repaints of the live terminal, which their reader follows.
      if (screenReaderMode) return
      if (!socket || socket.readyState !== WebSocket.OPEN) return
      if (performance.now() < historyProbeAt) return
      // Herdr must already render the grid the history view would use.
      if (!liveGridSettled()) return
      const controller = new AbortController()
      historyProbe = controller
      historyGeneration += 1
      const generation = historyGeneration
      const sizeAtProbe = `${targetSize.cols}x${targetSize.rows}`
      readHistory(historyEntryLines(targetSize.rows), 'ansi', controller.signal).then(
        (read) => {
          if (historyProbe !== controller) return
          historyProbe = null
          historyProbeMaxOffset = read.scroll?.max_offset_from_bottom ?? null
          if (disposed || generation !== historyGeneration) return
          if (
            sizeAtProbe !== `${targetSize.cols}x${targetSize.rows}` ||
            !liveGridSettled() ||
            !historyModeEligible(read, targetSize.rows)
          ) {
            backOffHistoryProbe()
            return
          }
          historyProbeBackoff = HISTORY_PROBE_BACKOFF_MS
          historyEntering = true
          void enterHistory(read, generation).finally(() => {
            historyEntering = false
          })
        },
        () => {
          if (historyProbe !== controller) return
          historyProbe = null
          backOffHistoryProbe()
        },
      )
    }

    const liveGridSettled = () =>
      lastSentSize === `${targetSize.cols}x${targetSize.rows}` &&
      lastFrameWidth === targetSize.cols &&
      lastFrameHeight === targetSize.rows

    const startDeferredHistoryProbe = () => {
      if (!historyProbeAfterFrame) return
      historyProbeAfterFrame = false
      startHistoryProbe()
    }

    const allowHistoryExtendRetry = () => {
      historyExtended = false
      historyExtendAt = performance.now() + HISTORY_EXTEND_RETRY_MS
    }

    // At the top of the coloured rows, prepend Herdr's older lines (plain
    // text: Herdr colours only its latest 1,000 rows) up to Yard's cap.
    const extendHistory = () => {
      if (
        !history ||
        !historyTruncated ||
        historyExtended ||
        historyExtending ||
        historyFilling
      ) {
        return
      }
      if (performance.now() < historyExtendAt) return
      const current = history
      const generation = historyGeneration
      const controller = new AbortController()
      historyExtending = controller
      historyExtended = true
      setHistoryNote('loading older output')
      readHistory(HISTORY_MAX_LINES, 'text', controller.signal)
        .then((read) =>
          withHistoryLock(async () => {
            if (historyExtending !== controller) return
            if (generation !== historyGeneration || history !== current) return
            historyExtending = null
            const older = read.text.replace(/\r\n?/g, '\n').split('\n')
            // Yard answers with only the capped rows, still truncated, when
            // Herdr's longer read failed (a busy pane changed mid-read): try
            // again at the next top reach.
            if (read.truncated && older.length <= HISTORY_ROW_LINES) {
              allowHistoryExtendRetry()
              setHistoryNote('older output not read yet; scroll up to retry')
              return
            }
            const seam = olderHistorySeam(
              older,
              historyRecords.map((record) => record.text),
              current.cols,
            )
            if (!seam || seam.olderEnd === 0) {
              setHistoryNote('no older output to add')
              return
            }
            const records = historyRecords.slice(seam.rowsStart)
            // Keep only the older lines that fit the view's scrollback with
            // the rendered rows; xterm would otherwise drop its top rows.
            const olderStart = olderLinesStart(
              older.slice(0, seam.olderEnd),
              current.cols,
              HISTORY_SCROLLBACK_ROWS - records.length,
            )
            const view = newHistoryView()
            await view.load(
              older
                .slice(olderStart, seam.olderEnd)
                .map((text) => ({ text, wraps: true })),
            )
            await view.load(records.map((record) => ({ text: record.ansi })))
            await view.settle()
            if (
              disposed ||
              generation !== historyGeneration ||
              !isHistoryView(current)
            ) {
              view.dispose()
              return
            }
            // Keep the rows in view where they are: the rendered rows now
            // start after the older lines, without the rows before the
            // join (the older lines hold them).
            const topIndex =
              current.topRow() -
              (current.bufferRows() - historyRecords.length)
            historyRecords = records
            // Where the rendered rows start, as written (older lines wrap).
            view.scrollToRow(historyBufferRow(view, 0) + topIndex - seam.rowsStart)
            showHistoryView(view)
            setHistoryNote(
              olderStart > 0
                ? `oldest ${olderStart.toLocaleString('en-US')} lines not kept; older lines without colour`
                : read.truncated
                  ? `latest ${HISTORY_MAX_LINES.toLocaleString('en-US')} lines; older lines without colour`
                  : 'older lines without colour',
            )
          }),
        )
        .catch(() => {
          if (historyExtending !== controller) return
          historyExtending = null
          allowHistoryExtendRetry()
          setHistoryNote('older output unavailable; scroll up to retry')
        })
    }

    // The cell under the pointer at the last wheel event, inside Herdr's
    // grid even during the renderer nudge. Measured only when a batch is
    // sent, so a burst of wheel events forces no layout.
    const wheelCell = (): TerminalCell | null => {
      const screen = host.querySelector('.xterm-screen')
      if (!screen || !lastWheelPoint) return null
      const cell = pointerTerminalCell(
        lastWheelPoint.x,
        lastWheelPoint.y,
        screen.getBoundingClientRect(),
        { cols: terminal.cols, rows: terminal.rows },
      )
      return {
        column: Math.min(cell.column, targetSize.cols - 1),
        row: Math.min(cell.row, targetSize.rows - 1),
      }
    }

    const clearScrollTimeout = () => {
      if (scrollTimeoutTimer) window.clearTimeout(scrollTimeoutTimer)
      scrollTimeoutTimer = 0
    }

    const resetScrollGate = () => {
      scrollGate = clearScrollGate(scrollGate)
      clearScrollTimeout()
    }

    // Send the pending scroll input as one batch if none is in flight. The
    // batch is answered by the first frame Yard read after the batch reached
    // Herdr (see answerScrollBatch) or by the timeout, and whatever
    // accumulated meanwhile goes next.
    const pumpScroll = () => {
      if (disposed) return
      if (!socket || socket.readyState !== WebSocket.OPEN) {
        resetScrollGate()
        return
      }
      const batch = takeScrollBatch(
        scrollGate,
        performance.now(),
        lastSequence,
        scrollMessagesSent,
      )
      if (batch.commands.length === 0) return
      scrollGate = batch.gate
      const cell = batch.commands.some((command) => command.source === 'wheel')
        ? wheelCell()
        : null
      inFlightScrollLines = 0
      for (const message of scrollBatchMessages(
        batch.commands,
        cell,
        targetSize.rows,
      )) {
        if (!sendScroll(message)) break
        inFlightScrollLines +=
          (message.direction === 'up' ? -1 : 1) *
          message.lines *
          (message.count ?? 1)
      }
      // A scroll-up is on its way to Herdr: read history right behind it.
      // For a pane that last showed a full-screen app, the read waits until
      // Herdr has answered the scroll: a response that reached the browser
      // first would push Herdr's frame past the tunnel's first congestion
      // window and delay the native scroll by a round trip. If the app has
      // since exited, history mode opens one round trip later.
      if (inFlightScrollLines < 0) {
        if (historyProbeMaxOffset === 0) historyProbeAfterFrame = true
        else startHistoryProbe()
      }
      clearScrollTimeout()
      scrollTimeoutTimer = window.setTimeout(() => {
        scrollTimeoutTimer = 0
        if (disposed) return
        scrollGate = expireScrollBatch(scrollGate)
        startDeferredHistoryProbe()
        pumpScroll()
      }, SCROLL_BATCH_TIMEOUT_MS)
    }

    const inputSubscription = terminal.onData((text) => {
      // Typing leaves history mode and reaches the pane as always.
      exitHistory()
      resetHistoryProbe()
      if (socket) {
        for (const chunk of chunkTerminalInput(text)) {
          if (!sendMessage(socket, { type: 'terminal.input', text: chunk })) {
            break
          }
          clearScrolledBack()
          resetScrollGate()
        }
      }
    })

    // PageUp/PageDown (plain or with Shift) scroll through Herdr. Every other
    // key stays ordinary xterm input.
    terminal.attachCustomKeyEventHandler((event) => {
      if (history) {
        // In history mode PageUp/PageDown page locally and Escape returns
        // to the live terminal; any other key is typed input (onData).
        const plain =
          !event.altKey && !event.ctrlKey && !event.metaKey && !event.shiftKey
        if (event.key === 'Escape' && plain) {
          // Consume the key so the workspace shell's Escape-to-close only
          // sees an Escape typed in the live terminal.
          event.stopPropagation()
          if (event.type === 'keydown') {
            event.preventDefault()
            exitHistory()
          }
          return false
        }
        const page = pageScrollMessage(event, terminal.rows)
        if (!page) return true
        if (event.type === 'keydown') {
          event.preventDefault()
          history.page(page.direction, () => {
            exitHistory()
          })
          if (history?.atTop()) extendHistory()
        }
        return false
      }
      const message = pageScrollMessage(event, terminal.rows)
      if (!message) return true
      if (event.type === 'keydown') {
        event.preventDefault()
        scrollGate = addPageScroll(scrollGate, message.direction)
        pumpScroll()
      }
      return false
    })

    // The wheel always goes to Herdr, which scrolls the app (mouse-aware
    // TUIs such as full-screen Claude Code get wheel reports) or its own
    // pane scrollback, then sends the repainted viewport. Each step is one
    // notch-sized command, because Herdr turns each command into exactly one
    // wheel report for a mouse-aware app. The first batch leaves at once;
    // later input is summed until Herdr answers.
    const handleWheel = (event: WheelEvent) => {
      // The history view scrolls itself, locally.
      if (history?.element.contains(event.target as Node)) return
      if (event.deltaY === 0 || event.ctrlKey) return
      event.preventDefault()
      event.stopPropagation()
      lastWheelPoint = { x: event.clientX, y: event.clientY }
      scrollGate = addWheelSteps(
        scrollGate,
        wheelEventSteps(event.deltaY, event.deltaMode, terminal.rows),
      )
      pumpScroll()
    }
    host.addEventListener('wheel', handleWheel, {
      capture: true,
      passive: false,
    })

    // A paste is one bracketed `terminal.input`, so the agent sees a single
    // paste rather than one submitted line per newline. It runs in the
    // capture phase so xterm never also sends it unbracketed and chunked.
    const handlePaste = (event: ClipboardEvent) => {
      const clipboardText = event.clipboardData?.getData('text/plain') ?? ''
      const paste = bracketedTerminalPaste(clipboardText)
      if (!paste) return
      event.preventDefault()
      event.stopPropagation()
      if (paste.kind === 'too_large') {
        setInputNotice(
          `Paste not sent: ${formatTerminalBytes(paste.bytes)} exceeds the ${formatTerminalBytes(paste.limit)} terminal paste limit`,
        )
        return
    }
      if (
        !socket ||
        !sendMessage(socket, { type: 'terminal.input', text: paste.text })
      ) {
        setInputNotice('Paste not sent: the terminal is not connected')
        return
      }
      exitHistory()
      resetHistoryProbe()
      clearScrolledBack()
      resetScrollGate()
      setInputNotice(null)
    }
    host.addEventListener('paste', handlePaste, { capture: true })

    // Copy in history mode copies the history selection. Focus stays on the
    // live terminal (so typing goes to the pane), whose own copy handler
    // would otherwise copy its empty selection.
    const handleCopy = (event: ClipboardEvent) => {
      const text = history?.selection()
      if (!text || !event.clipboardData) return
      event.clipboardData.setData('text/plain', text)
      event.preventDefault()
      event.stopImmediatePropagation()
    }
    host.addEventListener('copy', handleCopy, { capture: true })

    // A click in the history view (to select) must not take keyboard focus
    // from the live terminal.
    const handleFocusIn = (event: FocusEvent) => {
      if (history?.element.contains(event.target as Node)) terminal.focus()
    }
    host.addEventListener('focusin', handleFocusIn)

    jumpToLatestRef.current = () => {
      exitHistory()
      if (socket && sendMessage(socket, { type: 'terminal.scroll_reset' })) {
        clearScrolledBack()
        resetScrollGate()
      }
      terminal.focus()
    }

    const closeForInvalidFrame = (detail: string) => {
      setConnection({ kind: 'error', detail })
      socket?.close(1000, 'Invalid terminal frame')
    }

    // xterm appends every printed character to its screen-reader live region
    // and empties it only on a key press or blur. Herdr frames reprint the
    // whole screen with no line feeds, so drop what earlier frames added.
    const clearLiveRegion = () => {
      const liveRegion = host.querySelector('.live-region')
      if (liveRegion?.firstChild) liveRegion.textContent = ''
    }

    const queueFrame = (message: TerminalFrameMessage) => {
      let bytes: Uint8Array
      try {
        bytes = decodeBase64(message.bytes)
      } catch {
        closeForInvalidFrame('Terminal frame could not be decoded')
        return
      }
      const frameBytes = wrapHerdrFrame(bytes)
      frameWriteQueue = frameWriteQueue.then(async () => {
        if (disposed) return
        await writeTerminal(terminal, frameBytes)
        if (disposed) return
        if (screenReaderMode) clearLiveRegion()
        const waiting = scrollGate.inFlight
        scrollGate = answerScrollBatch(scrollGate, message, performance.now())
        if (waiting && !scrollGate.inFlight) {
          inFlightScrollLines = 0
          clearScrollTimeout()
          startDeferredHistoryProbe()
          pumpScroll()
        }
        scheduleHistoryMerge()
        lastFrameWidth = message.width
        lastFrameHeight = message.height
        pendingFrame = {
          seq: message.seq,
          width: message.width,
          height: message.height,
          full: message.full,
        }
        if (!frameAnimationFrame) {
          frameAnimationFrame = window.requestAnimationFrame(() => {
            frameAnimationFrame = 0
            if (!disposed && pendingFrame) {
              setFrame(pendingFrame)
              setBufferLines(terminal.buffer.active.length)
            }
            pendingFrame = null
          })
        }
      })
    }

    const acceptFrame = (message: TerminalFrameMessage) => {
      if (message.seq <= lastSequence) return
      lastSequence = message.seq
      queueFrame(message)
    }

    connectAnimationFrame = window.requestAnimationFrame(() => {
      // A single animation frame after this effect runs can still land in
      // the same layout pass that just made the (previously hidden or
      // unmounted) dialog visible, before its flexed size has settled —
      // a second frame guarantees layout has actually completed first.
      connectAnimationFrame = window.requestAnimationFrame(() => {
        connectAnimationFrame = 0
        if (disposed) return

        fitTerminal()
        // Capture the settled size before nudging — nudgeRendererResync()
        // leaves terminal.cols/rows sitting at a temporary value until its
        // own next-frame callback restores them.
        const { cols, rows } = targetSize
        nudgeRendererResync()
        // xterm's character-cell measurement (which the renderer's pixel
        // dimensions derive from) isn't guaranteed to be ready this soon
        // after the container becomes visible. Re-fit once more after a
        // real amount of clock time as a backstop, since animation frames
        // alone haven't been reliably long enough. The fit runs before the
        // nudge so a resize is sent only if the real size changed.
        settleFitTimer = window.setTimeout(() => {
          settleFitTimer = 0
          if (disposed) return
          fitTerminal()
          nudgeRendererResync()
          sendResize()
        }, 500)

        const connect = () => {
          const url = targetKind === 'yard-orchestrator'
            ? yardOrchestratorTerminalWebSocketUrl(cols, rows)
            : targetKind === 'coordination-node' && nodeId
              ? coordinationNodeTerminalWebSocketUrl(nodeId, cols, rows)
            : targetKind === 'assignment' && assignmentId && projectId
              ? assignmentTerminalWebSocketUrl(
                  projectId,
                  assignmentId,
                  cols,
                  rows,
                )
              : orchestratorTerminalWebSocketUrl(
                  projectId ?? '',
                  cols,
                  rows,
                )
          socket = new WebSocket(url)

          socket.onopen = () => {
            if (disposed) return
            reconnectAttemptRef.current = 0
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
              acceptFrame(message)
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
            scheduleReconnect()
          }

          socket.onclose = (event) => {
            if (disposed) return
            dropHistoryProbe()
            exitHistory()
            if (!released) scheduleReconnect()
            setConnection((current) => {
              if (
                reconnectScheduled ||
                current.kind === 'closed' ||
                current.kind === 'error'
              ) {
                return current
              }
              return {
                kind: 'closed',
                detail: event.reason || 'Terminal connection closed',
              }
            })
          }
        }

        connect()
      })
    })

    return () => {
      disposed = true
      resizeObserver.disconnect()
      inputSubscription.dispose()
      resizeSubscription.dispose()
      terminalThemeHandlers.forEach((handler) => handler.dispose())
      window.removeEventListener(THEME_CHANGE_EVENT, handleThemeChange)
      host.removeEventListener('wheel', handleWheel, true)
      host.removeEventListener('paste', handlePaste, true)
      host.removeEventListener('copy', handleCopy, true)
      host.removeEventListener('focusin', handleFocusIn)
      jumpToLatestRef.current = null
      historyGeneration += 1
      historyProbe?.abort()
      historyExtending?.abort()
      historyRefreshing?.abort()
      historyFilling?.abort()
      if (historyMergeFrame) window.cancelAnimationFrame(historyMergeFrame)
      history?.dispose()
      history = null
      historyViewRef.current = null
      if (fitAnimationFrame) {
        window.cancelAnimationFrame(fitAnimationFrame)
      }
      clearScrollTimeout()
      if (settleFitTimer) {
        window.clearTimeout(settleFitTimer)
      }
      if (resizeSendTimer) {
        window.clearTimeout(resizeSendTimer)
      }
      if (connectAnimationFrame) {
        window.cancelAnimationFrame(connectAnimationFrame)
      }
      if (frameAnimationFrame) {
        window.cancelAnimationFrame(frameAnimationFrame)
      }
      if (reconnectTimer) {
        window.clearTimeout(reconnectTimer)
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
      terminalRef.current = null
      terminal.dispose()
    }
  }, [
    assignmentId,
    connectionAttempt,
    nodeId,
    projectId,
    targetKind,
    workerId,
  ])

  // Apply the selected app theme live without reconnecting or losing scrollback.
  useEffect(() => {
    const terminal = terminalRef.current
    if (!terminal) return
    applyResolvedTerminalTheme(terminal, theme)
    historyViewRef.current?.setTheme(terminalTheme(theme))
    const refreshFrame = window.requestAnimationFrame(() => {
      if (terminal.rows > 0) terminal.refresh(0, terminal.rows - 1)
    })
    return () => window.cancelAnimationFrame(refreshFrame)
  }, [theme])

  const inHistory = connection.kind === 'connected' && historyMode === 'history'
  return (
    <div
      className="terminal-session"
      data-frame-full={frame?.full}
      data-frame-height={frame?.height}
      data-frame-sequence={frame?.seq}
      data-frame-width={frame?.width}
      data-state={connection.kind}
      data-terminal-buffer-lines={bufferLines ?? undefined}
      data-terminal-cols={terminalSize?.cols}
      data-terminal-rows={terminalSize?.rows}
      data-terminal-theme={theme}
      data-terminal-view={historyMode}
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
      <div className="terminal-session__status">
        <div
          aria-live="polite"
          className="terminal-session__status-text"
          role="status"
          title={
            connection.kind === 'connected'
              ? 'Scroll or press PageUp for earlier output'
              : undefined
          }
        >
          <span aria-hidden="true" />
          {inHistory
            ? 'History — scroll down or type to return to live'
            : connection.detail}
          {inHistory && historyNote ? ` · ${historyNote}` : null}
          {inHistory && historyTail === 'updating'
            ? ' · adding newer output'
            : null}
          {inHistory && historyTail === 'behind'
            ? ' · newer output not shown; Jump to latest'
            : null}
          {!inHistory && connection.kind === 'connected' && scrolledBack
            ? ' · Scrolled back'
            : null}
        </div>
        {inHistory || (connection.kind === 'connected' && scrolledBack) ? (
          <button
            className="terminal-session__latest"
            onClick={() => jumpToLatestRef.current?.()}
            title="Return to the latest output"
            type="button"
          >
            <ArrowDownToLine aria-hidden="true" size={13} />
            Jump to latest
          </button>
        ) : null}
        {inputNotice ? (
          <div
            className="terminal-session__notice"
            role="alert"
            title={inputNotice}
          >
            {inputNotice}
        </div>
        ) : null}
        {connection.kind === 'closed' || connection.kind === 'error' ? (
          <button
            aria-label="Reconnect terminal"
            className="terminal-session__reopen"
            onClick={() => {
              reconnectAttemptRef.current = 0
              setConnectionAttempt((attempt) => attempt + 1)
            }}
            title="Reconnect terminal"
            type="button"
          >
            <RefreshCw aria-hidden="true" size={13} />
          </button>
        ) : null}
      </div>
    </div>
  )
}

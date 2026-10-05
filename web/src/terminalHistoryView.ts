import { type ITheme, Terminal } from '@xterm/xterm'
import { HISTORY_SCROLLBACK_ROWS } from './terminalHistory'

// Writes of history are split so that no single parse (xterm parses one
// write at a time) becomes a long task.
const HISTORY_WRITE_CHUNK_CHARS = 32 * 1024

const AUTOWRAP_OFF = '\u001b[?7l'
const AUTOWRAP_ON = '\u001b[?7h'
const HIDE_CURSOR = '\u001b[?25l'

export interface HistoryViewOptions {
  host: HTMLElement
  cols: number
  rows: number
  theme: ITheme
  /** A wheel or page step down while the view already shows its last row. */
  onPastBottom: () => void
  /** The view reached its first row. */
  onTop: () => void
}

/** A history row to write: ANSI text, and whether it may wrap. */
export interface HistoryLine {
  text: string
  /** Plain logical lines (older history) wrap; rendered rows never do. */
  wraps?: boolean
}

function write(terminal: Terminal, data: string) {
  return new Promise<void>((resolve) => terminal.write(data, resolve))
}

/**
 * The local, read-only xterm that shows a pane's history. It sits over the
 * live terminal with the same grid, has its own scrollback, and scrolls
 * without any network traffic. Buffer row `i` holds history row `i`
 * (older plain lines may take several rows; `rowOffset` is where the
 * rendered rows start).
 */
export class TerminalHistoryView {
  readonly element: HTMLDivElement
  readonly terminal: Terminal
  readonly cols: number
  readonly rows: number
  private disposed = false
  private written = false
  private readonly handleWheel: (event: WheelEvent) => void

  constructor(options: HistoryViewOptions) {
    this.cols = options.cols
    this.rows = options.rows
    this.element = document.createElement('div')
    this.element.className = 'terminal-session__history'
    this.element.dataset.ready = 'false'
    options.host.appendChild(this.element)
    this.terminal = new Terminal({
      cols: options.cols,
      convertEol: false,
      cursorBlink: false,
      cursorInactiveStyle: 'none',
      cursorStyle: 'block',
      disableStdin: true,
      fontFamily: '"IBM Plex Mono", monospace',
      fontSize: 11,
      letterSpacing: 0,
      lineHeight: 1.2,
      minimumContrastRatio: 4.5,
      rows: options.rows,
      screenReaderMode: false,
      scrollback: HISTORY_SCROLLBACK_ROWS,
      smoothScrollDuration: 0,
      theme: options.theme,
    })
    this.terminal.open(this.element)
    this.handleWheel = (event: WheelEvent) => {
      if (event.deltaY > 0 && this.atBottom()) {
        event.preventDefault()
        event.stopPropagation()
        options.onPastBottom()
      } else if (event.deltaY < 0 && this.atTop()) {
        options.onTop()
      }
    }
    this.element.addEventListener('wheel', this.handleWheel, {
      capture: true,
      passive: false,
    })
    this.terminal.onScroll(() => {
      if (this.disposed) return
      this.describe()
      if (this.atTop()) options.onTop()
    })
  }

  // Test and debugging hooks: buffer rows in use and the first row shown.
  private describe() {
    this.element.dataset.rows = String(this.bufferRows())
    this.element.dataset.top = String(this.topRow())
  }

  /**
   * Append history lines below what is already written, in chunks, then
   * hide the cursor. Resolves once xterm has parsed them.
   */
  async load(lines: readonly HistoryLine[]) {
    let chunk = ''
    let wraps: boolean | null = null
    const flush = async () => {
      if (!chunk || this.disposed) return
      const data = chunk
      chunk = ''
      await write(this.terminal, data)
    }
    for (let index = 0; index < lines.length; index += 1) {
      const line = lines[index]
      const lineWraps = line.wraps === true
      if (lineWraps !== wraps) {
        chunk += lineWraps ? AUTOWRAP_ON : AUTOWRAP_OFF
        wraps = lineWraps
      }
      chunk += this.written ? `\r\n${line.text}` : line.text
      this.written = true
      if (chunk.length >= HISTORY_WRITE_CHUNK_CHARS) await flush()
      if (this.disposed) return
    }
    chunk += `${AUTOWRAP_OFF}${HIDE_CURSOR}`
    await flush()
    if (!this.disposed) this.describe()
  }

  /**
   * Wait until xterm has synced its scroll area to the rows written (it
   * does so on its next render frames); positioning before that can clamp
   * to the old height.
   */
  settle() {
    return new Promise<void>((resolve) =>
      window.requestAnimationFrame(() =>
        window.requestAnimationFrame(() => resolve()),
      ),
    )
  }

  /** Buffer rows in use: everything up to the row the last line ended on. */
  bufferRows() {
    const buffer = this.terminal.buffer.active
    return this.written ? buffer.baseY + buffer.cursorY + 1 : 0
  }

  atBottom() {
    const buffer = this.terminal.buffer.active
    return buffer.viewportY >= buffer.baseY
  }

  atTop() {
    return this.terminal.buffer.active.viewportY === 0
  }

  topRow() {
    return this.terminal.buffer.active.viewportY
  }

  scrollToRow(row: number) {
    this.terminal.scrollToLine(Math.max(0, row))
    this.describe()
  }

  scrollLines(lines: number) {
    if (lines !== 0) this.terminal.scrollLines(lines)
    this.describe()
  }

  /** PageUp/PageDown inside history; a page down at the bottom leaves. */
  page(direction: 'up' | 'down', onPastBottom: () => void) {
    if (direction === 'down' && this.atBottom()) {
      onPastBottom()
      return
    }
    this.terminal.scrollPages(direction === 'up' ? -1 : 1)
  }

  /**
   * Replace the rows from buffer row `fromRow` (inside the last screen of
   * the buffer) with `rows`, appending below as needed. The view keeps its
   * position unless it was already following the bottom.
   */
  async replaceTail(fromRow: number, rows: readonly string[]) {
    const buffer = this.terminal.buffer.active
    const screenRow = fromRow - buffer.baseY
    if (screenRow < 0 || screenRow >= this.rows || this.disposed) return false
    // A refresh can bring up to a full read of rows: write it in chunks,
    // like load, so no single parse becomes a long task.
    let chunk = `${AUTOWRAP_OFF}\u001b[${screenRow + 1};1H\u001b[0m\u001b[J`
    for (let index = 0; index < rows.length; index += 1) {
      chunk += index === 0 ? rows[index] : `\r\n${rows[index]}`
      if (chunk.length >= HISTORY_WRITE_CHUNK_CHARS) {
        await write(this.terminal, chunk)
        chunk = ''
        if (this.disposed) return false
      }
    }
    await write(this.terminal, `${chunk}${HIDE_CURSOR}`)
    if (this.disposed) return false
    this.describe()
    return true
  }

  selection() {
    return this.terminal.hasSelection() ? this.terminal.getSelection() : ''
  }

  setTheme(theme: ITheme) {
    this.terminal.options.theme = { ...theme }
  }

  show() {
    this.element.dataset.ready = 'true'
  }

  dispose() {
    if (this.disposed) return
    this.disposed = true
    this.element.removeEventListener('wheel', this.handleWheel, true)
    this.terminal.dispose()
    this.element.remove()
  }
}

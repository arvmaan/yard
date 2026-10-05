// Pure helpers for the terminal's local history mode.
//
// Herdr owns a pane's scrollback, and every scroll step through it costs a
// network round trip. When Herdr (not the app) owns the wheel for a pane,
// Yard reads the pane's rendered rows once and shows them in a local,
// read-only xterm that scrolls at browser speed. The live terminal keeps
// applying Herdr's frames underneath and is never written with history.

// Herdr's `pane.read` renders at most its latest 1,000 rows, and only that
// read carries colours.
export const HISTORY_ROW_LINES = 1_000
// The probe right behind the first scroll-up reads only this many screens
// of rows, so history opens after one small response (a large one needs
// several round trips on a connection that was idle); the rest of the
// coloured rows load right after, in the background.
export const HISTORY_ENTRY_SCREENS = 2
export const historyEntryLines = (rows: number) =>
  Math.min(HISTORY_ROW_LINES, Math.max(1, rows) * HISTORY_ENTRY_SCREENS)
// Yard's terminal-output cap. Rows older than the coloured ones come from
// Herdr's selection read, which is plain text.
export const HISTORY_MAX_LINES = 10_000
// A seam or a located screen must agree on at least this many rows (or on
// every row the two share) before Yard trusts it.
export const MIN_SEAM_ROWS = 3
// After a read says the pane is not eligible, wait this long (doubling up
// to the maximum) before probing again. Input resets it.
export const HISTORY_PROBE_BACKOFF_MS = 2_000
export const MAX_HISTORY_PROBE_BACKOFF_MS = 30_000
// While in history, a fresh read of the latest rows is joined on at most
// this often when the live screen no longer joins the history.
export const HISTORY_REFRESH_INTERVAL_MS = 1_000
// An older-lines read that found nothing older (Herdr's longer read failed)
// is retried at the next top reach after this long.
export const HISTORY_EXTEND_RETRY_MS = 1_000
// Rows the history xterm keeps: the full read, older plain lines, and the
// live screens merged below them.
export const HISTORY_SCROLLBACK_ROWS = HISTORY_MAX_LINES + 2_000

export interface HistoryScrollPosition {
  offset_from_bottom: number
  max_offset_from_bottom: number
  viewport_rows: number
}

export interface HistoryRead {
  format: string
  text: string
  truncated: boolean
  scroll?: HistoryScrollPosition | null
}

/**
 * Whether history mode applies to the pane this read came from.
 *
 * Yard asks for the read right after sending its first scroll-up through
 * Herdr, and Herdr reports its scroll position after the rows. Herdr moves
 * that position only when it applies a scroll to its own scrollback; for a
 * mouse-reporting app (full-screen Claude Code, even an inline app that
 * captures the mouse) or an alternate-scroll app (`less`) it forwards the
 * wheel and resets the position to 0, and an alternate screen has no
 * scrollback at all (`max_offset_from_bottom` 0). So a position above 0 is
 * Herdr saying "I scrolled my own history for this pane". The rows must also
 * be laid out for the live terminal's height.
 */
export function historyModeEligible(read: HistoryRead, viewportRows: number) {
  const scroll = read.scroll
  return (
    read.format === 'ansi' &&
    !!scroll &&
    scroll.max_offset_from_bottom > 0 &&
    scroll.offset_from_bottom > 0 &&
    scroll.viewport_rows === viewportRows
  )
}

/** Herdr's `recent` ANSI read: one rendered row per `\r\n` line. */
export function historyRows(text: string): string[] {
  return text ? text.split(/\r?\n/) : []
}

// CSI, OSC (BEL or ST terminated), and two-byte escape sequences.
const ESCAPE_SEQUENCE =
  // eslint-disable-next-line no-control-regex
  /\u001b(?:\[[0-?]*[ -/]*[@-~]|\][^\u0007\u001b]*(?:\u0007|\u001b\\)|[@-Z\\-_])/g

export function stripTerminalEscapes(text: string) {
  return text.replace(ESCAPE_SEQUENCE, '')
}

/**
 * The comparison key for one row. Whitespace and control characters are
 * ignored: Herdr positions a wide character's successor with CUP, so the
 * live screen can hold a padding cell where the history row has none.
 */
export function rowKey(text: string) {
  // eslint-disable-next-line no-control-regex
  return text.replace(/[\s\u0000-\u001f\u007f]+/g, '')
}

export function historyRowKey(row: string) {
  return rowKey(stripTerminalEscapes(row))
}

function contentRows(keys: readonly string[]) {
  let end = keys.length
  while (end > 0 && keys[end - 1] === '') end -= 1
  return end
}

/** Leading rows of `rows` that equal `history` from `top` onward. */
function matchedRun(
  history: readonly string[],
  top: number,
  rows: readonly string[],
  count: number,
) {
  let run = 0
  while (
    run < count &&
    top + run < history.length &&
    history[top + run] === rows[run]
  ) {
    run += 1
  }
  return run
}

/**
 * A trusted alignment: every row the two share agrees, or at least
 * `MIN_SEAM_ROWS` leading rows do (an inline app may have redrawn its
 * footer since), and some agreeing row has text.
 */
function trustedRun(
  history: readonly string[],
  top: number,
  rows: readonly string[],
  count: number,
) {
  const run = matchedRun(history, top, rows, count)
  const shared = Math.min(count, history.length - top)
  if (run === 0 || (run < shared && run < MIN_SEAM_ROWS)) return 0
  for (let index = 0; index < run; index += 1) {
    if (rows[index] !== '') return run
  }
  return 0
}

interface Alignment {
  top: number
  run: number
}

function bestAlignment(
  history: readonly string[],
  rows: readonly string[],
  from: number,
  to: number,
  preferTop: number,
): Alignment | null {
  const count = contentRows(rows)
  if (count === 0) return null
  let best: Alignment | null = null
  for (let top = Math.max(0, from); top < Math.min(history.length, to); top += 1) {
    if (history[top] !== rows[0]) continue
    const run = trustedRun(history, top, rows, count)
    if (run === 0) continue
    if (
      !best ||
      run > best.run ||
      (run === best.run &&
        Math.abs(top - preferTop) < Math.abs(best.top - preferTop))
    ) {
      best = { top, run }
    }
  }
  return best
}

/**
 * The history row where the rows on screen (`view`, possibly Herdr's
 * scrolled-back view) start, so history mode opens on exactly what the
 * user already sees. Ties go to the match nearest `expectedTop`.
 */
export function locateRows(
  history: readonly string[],
  view: readonly string[],
  expectedTop: number,
): number | null {
  return bestAlignment(history, view, 0, history.length, expectedTop)?.top ?? null
}

/**
 * Where the live screen (Herdr's latest screen, `screen.length` rows)
 * begins in the history, so history can end with exactly that screen:
 * history rows from the returned index on are the screen's rows (or an
 * older paint of them) and are replaced by it, and nothing before it is
 * dropped or repeated.
 *
 * The screen's top row can only sit within the last screen of history (it
 * moves down as output arrives). Returns null when there is no trusted
 * overlap (more than a screen of new output, a full redraw), or when the
 * screen matches an earlier history position at least as well (repeated
 * content, or a frame of Herdr's scrolled-back view): merging then could
 * duplicate or drop rows, so the history stays as read.
 */
export function screenSeam(
  history: readonly string[],
  screen: readonly string[],
): number | null {
  const windowStart = Math.max(0, history.length - screen.length)
  const seam = bestAlignment(history, screen, windowStart, history.length, 0)
  if (!seam) return null
  const earlier = bestAlignment(history, screen, 0, windowStart, windowStart)
  if (earlier && earlier.run >= seam.run) return null
  // Two window rows that each agree with the screen on every row they
  // share mean the history ends with repeated rows (a line printed again
  // and again): the screen could continue it from either, and picking the
  // wrong one drops or repeats rows.
  const count = contentRows(screen)
  let full = 0
  for (let top = windowStart; top < history.length; top += 1) {
    if (history[top] !== screen[0]) continue
    const run = trustedRun(history, top, screen, count)
    if (run > 0 && run === Math.min(count, history.length - top)) full += 1
    if (full > 1) return null
  }
  return seam.top
}

/**
 * Where a fresh read of Herdr's latest rows (`rows`) continues the history,
 * for when the live screen alone no longer joins it (more than a screen of
 * output arrived between two frames). History rows from `seam` on are
 * replaced by `rows` from `from` on.
 *
 * Every history row before its last screen is final, so the read must agree
 * with all of them from where it starts; only the last `screenRows` rows
 * may be an older paint (a stale prompt or status line). Returns null when
 * the read does not reach back into the history (more new output than one
 * read holds), or when it could start at two places (repeated content).
 */
export function refreshSeam(
  history: readonly string[],
  rows: readonly string[],
  screenRows: number,
): { seam: number; from: number } | null {
  if (rows.length === 0) return null
  const stableEnd = Math.max(0, history.length - screenRows)
  // A start inside the final rows is checked against them; one inside the
  // last screen has only its own run to go on, so it needs a full run and
  // counts only when no final row supports another start.
  let verified: { seam: number; from: number } | null = null
  let unverified: { seam: number; from: number } | null = null
  let ambiguous = false
  for (let top = 0; top < history.length; top += 1) {
    if (history[top] !== rows[0]) continue
    const run = trustedRun(history, top, rows, rows.length)
    if (run === 0 || top + run < stableEnd) continue
    const join = { seam: top + run, from: run }
    if (top < stableEnd) {
      if (verified) return null
      verified = join
    } else if (run >= MIN_SEAM_ROWS) {
      if (unverified) ambiguous = true
      unverified = join
    }
  }
  if (verified) return verified
  return ambiguous ? null : unverified
}

/**
 * Where the history (`rows`) starts inside a longer, later read of Herdr's
 * rendered rows (`older`), so the read's earlier rows can go above it:
 * `older` rows before the returned index are older than the history's first
 * row. The two must agree on every final row both hold (the history's last
 * screen may be an older paint of rows Herdr has redrawn since). Returns null
 * when the history is not found, or is found at two places.
 */
export function prependSeam(
  older: readonly string[],
  rows: readonly string[],
  screenRows: number,
): number | null {
  if (rows.length === 0) return null
  const stable = Math.max(0, rows.length - screenRows)
  let found: number | null = null
  for (let top = 0; top < older.length; top += 1) {
    if (older[top] !== rows[0]) continue
    const run = trustedRun(older, top, rows, rows.length)
    if (run === 0 || run < Math.min(stable, older.length - top)) continue
    if (found !== null) return null
    found = top
  }
  return found
}

/** Replace the history's tail from `seam` on with the live screen. */
export function mergeScreen<Row>(
  history: readonly Row[],
  seam: number,
  screen: readonly Row[],
): Row[] {
  return [...history.slice(0, seam), ...screen]
}

function displayWidth(text: string) {
  return [...text].length
}

/**
 * Join older plain history onto the coloured rows. Herdr's older lines are
 * unwrapped logical lines; the coloured rows are rendered rows, where a
 * full-width row may continue on the next. A row narrower than the pane
 * cannot have wrapped, so the row after it starts a logical line, and a
 * narrow row is a whole logical line. The first run of such rows that also
 * appears exactly once in the older lines anchors the join: older lines
 * before the anchor, then the coloured rows from the anchor on. Returns null
 * when no such run is found, so no line is guessed.
 */
export function olderHistorySeam(
  olderLines: readonly string[],
  rowTexts: readonly string[],
  cols: number,
): { olderEnd: number; rowsStart: number } | null {
  const narrow = (text: string) => displayWidth(text) < cols
  const olderKeys = olderLines.map(rowKey)
  const searchRows = Math.min(rowTexts.length, 400)
  for (let start = 1; start < searchRows; start += 1) {
    if (!narrow(rowTexts[start - 1])) continue
    let count = 0
    while (
      start + count < rowTexts.length &&
      count < 8 &&
      narrow(rowTexts[start + count])
    ) {
      count += 1
    }
    if (count < MIN_SEAM_ROWS) continue
    const keys = rowTexts.slice(start, start + count).map(rowKey)
    if (keys.filter(Boolean).length < 2) continue
    // The run must appear exactly once in the older lines. A repeated
    // block (the same summary printed twice) could join at the wrong copy,
    // and rendered rows cannot say which copy is which (wrapped rows are
    // not logical lines), so try a later run instead.
    let found: number | null = null
    let repeated = false
    for (let index = 0; index + count <= olderKeys.length; index += 1) {
      if (olderKeys[index] !== keys[0]) continue
      let equal = true
      for (let offset = 1; offset < count; offset += 1) {
        if (olderKeys[index + offset] !== keys[offset]) {
          equal = false
          break
        }
      }
      if (!equal) continue
      if (found !== null) {
        repeated = true
        break
      }
      found = index
    }
    if (found !== null && !repeated) return { olderEnd: found, rowsStart: start }
  }
  return null
}

/**
 * The first of `lines` (older plain logical lines, which wrap at `cols`)
 * to keep so that the rows they take fit in `rowBudget`; the oldest lines
 * are the ones left out.
 */
export function olderLinesStart(
  lines: readonly string[],
  cols: number,
  rowBudget: number,
) {
  let rows = 0
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    const lineRows = Math.max(1, Math.ceil(displayWidth(lines[index]) / cols))
    if (rows + lineRows > rowBudget) return index + 1
    rows += lineRows
  }
  return 0
}

/** The cell API history mode needs from xterm's buffer. */
export interface SerializableCell {
  getChars(): string
  getWidth(): number
  getFgColor(): number
  getBgColor(): number
  isFgRGB(): boolean
  isBgRGB(): boolean
  isFgPalette(): boolean
  isBgPalette(): boolean
  isBold(): number
  isDim(): number
  isItalic(): number
  isUnderline(): number
  isBlink(): number
  isInverse(): number
  isInvisible(): number
  isStrikethrough(): number
  isOverline(): number
}

export interface SerializableLine {
  getCell(x: number, cell?: SerializableCell): SerializableCell | undefined
}

function colorParameter(base: 38 | 48, rgb: boolean, palette: boolean, color: number) {
  if (rgb) {
    return `${base};2;${(color >> 16) & 255};${(color >> 8) & 255};${color & 255}`
  }
  return palette ? `${base};5;${color}` : null
}

function cellStyle(cell: SerializableCell) {
  const parameters = ['0']
  if (cell.isBold()) parameters.push('1')
  if (cell.isDim()) parameters.push('2')
  if (cell.isItalic()) parameters.push('3')
  if (cell.isUnderline()) parameters.push('4')
  if (cell.isBlink()) parameters.push('5')
  if (cell.isInverse()) parameters.push('7')
  if (cell.isInvisible()) parameters.push('8')
  if (cell.isStrikethrough()) parameters.push('9')
  if (cell.isOverline()) parameters.push('53')
  const fg = colorParameter(38, cell.isFgRGB(), cell.isFgPalette(), cell.getFgColor())
  const bg = colorParameter(48, cell.isBgRGB(), cell.isBgPalette(), cell.getBgColor())
  if (fg) parameters.push(fg)
  if (bg) parameters.push(bg)
  return parameters.join(';')
}

/**
 * One live-screen row as text with SGR colours, for merging the live screen
 * into history. Trailing unstyled blanks are dropped.
 */
export function serializeRow(
  line: SerializableLine,
  cols: number,
  scratch?: SerializableCell,
): string {
  const pieces: string[] = []
  let style = '0'
  let end = 0
  let cell = scratch
  for (let x = 0; x < cols; x += 1) {
    cell = line.getCell(x, cell)
    if (!cell) break
    if (cell.getWidth() === 0) continue
    const next = cellStyle(cell)
    const chars = cell.getChars() || ' '
    if (next !== style) {
      pieces.push(`\u001b[${next}m`)
      style = next
    }
    pieces.push(chars)
    if (chars !== ' ' || next !== '0') end = pieces.length
  }
  const text = pieces.slice(0, end).join('')
  return text.includes('\u001b[') ? `${text}\u001b[0m` : text
}

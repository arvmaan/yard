// Pure helpers for the Yard ⇄ Herdr interactive terminal protocol.
//
// Herdr's `terminal session control` stream is not a PTY byte stream: every
// frame is a cursor-positioned repaint of a fixed cols × rows screen. Nothing
// ever scrolls into the browser's buffer, so earlier output lives in Herdr
// (its pane scrollback, or a full-screen app's own transcript) and is reached
// by sending `terminal.scroll`, which makes Herdr repaint the viewport.

import type { TerminalScrollMessage } from './types'

// Mirrors `MIN/MAX_TERMINAL_COLS/ROWS` in yard-server's terminal_service.rs.
export const MIN_TERMINAL_COLS = 20
export const MAX_TERMINAL_COLS = 400
export const MIN_TERMINAL_ROWS = 5
export const MAX_TERMINAL_ROWS = 200

// Mirrors `MAX_TERMINAL_INPUT_BYTES` in yard-server's terminal_service.rs.
export const MAX_TERMINAL_PASTE_BYTES = 512 * 1024

// Browser pixel deltas per wheel step (one notch) for trackpads and
// pixel-mode wheels. xterm sends a mouse-aware app one wheel report per
// ~44 px of trackpad motion (one ~13 px cell, damped by 0.3), so a step here
// matches what the app would get from a local xterm.
export const WHEEL_PIXELS_PER_STEP = 44

// Lines per wheel `terminal.scroll`, matching Herdr's default
// `mouse_scroll_lines` (one wheel event from its attach client). Herdr uses
// `lines` only for its own scrollback; for mouse-reporting and
// alternate-scroll apps each command becomes exactly one wheel report or
// arrow key, so every step is its own command.
export const WHEEL_LINES_PER_SCROLL = 3

// Flow control. Every scroll answer is a Herdr repaint that costs a network
// round trip. Input that arrives while the window below is full is summed
// (up to this many steps) and sent as the next batch when an answer arrives.
export const MAX_SCROLL_BATCH_COMMANDS = 5
export const MAX_PENDING_SCROLL_STEPS = 5

// Pipelining, once Yard's frame tags have measured the round trip: batches
// leave at most one per Herdr render interval (16 ms), the pace at which a
// local terminal would hand an app its wheel reports, and enough of them may
// be in flight to cover one round trip. With one batch per round trip, the
// view moved only once per round trip, a batch the app did not repaint for
// held the next one until the timeout, and full-screen Claude Code, which
// speeds up for fast wheel reports, scrolled several times further on a
// short round trip than on a long one.
export const SCROLL_SEND_INTERVAL_MS = 16
// Enough for a ~180 ms round trip at one batch per send interval.
export const MAX_SCROLL_BATCHES_IN_FLIGHT = 12
/**
 * Tagged answers the window's round trip is the fastest of. The minimum of a
 * few recent answers ignores one slow frame but follows a path that slows.
 */
export const ROUND_TRIP_SAMPLES = 8
// Answer bytes the window may have in flight: a pane whose every answer is a
// large repaint (a shell scrolling its scrollback) keeps a small window.
export const MAX_SCROLL_ANSWER_BYTES_IN_FLIGHT = 64 * 1024

// A batch that produces no frame (for example at the top of the scrollback)
// is treated as answered after this long.
export const SCROLL_BATCH_TIMEOUT_MS = 300

// Untagged frames only: a timeout never lowers the answer-latency estimate
// below this, so the half-latency guard stays above Herdr's ~17 ms gap
// between two renders of one batch (about 2x its 16 ms render interval).
export const MIN_UNTAGGED_ANSWER_ESTIMATE_MS = 40

// Opt-in for xterm's screen-reader mode. It is off by default: xterm
// announces every printed character, and Herdr's cursor-positioned repaints
// contain no line feeds, so its live region grew by a whole screen per scroll
// step until the browser stalled for seconds.
export const TERMINAL_SCREEN_READER_STORAGE_KEY =
  'yard:terminal-screen-reader:v1'

// Tolerance for float drift when summing pixel deltas (11 × 4/44 must still
// count as one whole step).
const WHEEL_STEP_EPSILON = 1e-6

export interface TerminalSize {
  cols: number
  rows: number
}

export interface TerminalCell {
  column: number
  row: number
}

function clampInteger(value: number, min: number, max: number) {
  if (!Number.isFinite(value)) return min
  return Math.min(max, Math.max(min, Math.floor(value)))
}

/** Clamp a fitted size to the range the Yard server and Herdr accept. */
export function clampTerminalSize(size: TerminalSize): TerminalSize {
  return {
    cols: clampInteger(size.cols, MIN_TERMINAL_COLS, MAX_TERMINAL_COLS),
    rows: clampInteger(size.rows, MIN_TERMINAL_ROWS, MAX_TERMINAL_ROWS),
  }
}

const AUTOWRAP_OFF = [0x1b, 0x5b, 0x3f, 0x37, 0x6c] // ESC [ ? 7 l
const AUTOWRAP_ON = [0x1b, 0x5b, 0x3f, 0x37, 0x68] // ESC [ ? 7 h

/**
 * Bracket one Herdr frame with autowrap off. Herdr positions every row with
 * CUP and never relies on wrapping, so a frame wider than the local view
 * (for example, between a local fit and Herdr's resized repaint) clips at the
 * right edge instead of wrapping into, and scrolling, the rows below it.
 */
export function wrapHerdrFrame(bytes: Uint8Array): Uint8Array {
  const wrapped = new Uint8Array(
    AUTOWRAP_OFF.length + bytes.length + AUTOWRAP_ON.length,
  )
  wrapped.set(AUTOWRAP_OFF, 0)
  wrapped.set(bytes, AUTOWRAP_OFF.length)
  wrapped.set(AUTOWRAP_ON, AUTOWRAP_OFF.length + bytes.length)
  return wrapped
}

const PASTE_START = '\u001b[200~'
const PASTE_END = '\u001b[201~'
const encoder = new TextEncoder()

export type TerminalPaste =
  | { kind: 'ready'; text: string; bytes: number }
  | { kind: 'too_large'; bytes: number; limit: number }

function wellFormed(text: string) {
  // A lone UTF-16 surrogate cannot be decoded by the server's JSON parser.
  return text.replace(
    /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g,
    '\uFFFD',
  )
}

/**
 * Build one bracketed paste for `terminal.input`.
 *
 * Newlines become carriage returns like a native terminal paste, embedded
 * paste markers are removed so pasted text cannot end the paste early, and
 * the result is bracketed so Herdr delivers it to the pane as one paste
 * (re-bracketed only if the app enabled bracketed paste) instead of a burst
 * of Enter key presses.
 */
export function bracketedTerminalPaste(
  clipboardText: string,
  limit = MAX_TERMINAL_PASTE_BYTES,
): TerminalPaste | null {
  const body = wellFormed(clipboardText)
    .split(PASTE_START)
    .join('')
    .split(PASTE_END)
    .join('')
    .replace(/\r?\n/g, '\r')
  if (!body) return null
  const text = `${PASTE_START}${body}${PASTE_END}`
  const bytes = encoder.encode(text).byteLength
  if (bytes > limit) return { kind: 'too_large', bytes, limit }
  return { kind: 'ready', text, bytes }
}

export function formatTerminalBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${Math.ceil(bytes / 1024)} KiB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`
}

/**
 * Signed, possibly fractional, steps for one wheel event (negative = up).
 * Pixel deltas add up at one step per `WHEEL_PIXELS_PER_STEP`, so slow
 * trackpad motion is never dropped. A discrete line- or page-mode wheel event
 * is always at least one step.
 */
export function wheelEventSteps(
  deltaY: number,
  deltaMode: number,
  viewportRows: number,
) {
  if (!Number.isFinite(deltaY) || deltaY === 0) return 0
  if (deltaMode === 0) return deltaY / WHEEL_PIXELS_PER_STEP // DOM_DELTA_PIXEL
  if (deltaMode === 2) {
    // DOM_DELTA_PAGE: a screen per unit, always at least one step.
    const lines = deltaY * Math.max(1, viewportRows)
    return (
      Math.sign(lines) * Math.max(1, Math.abs(lines) / WHEEL_LINES_PER_SCROLL)
    )
  }
  // DOM_DELTA_LINE: a notch of a line or more is at least one step; a
  // sub-line delta (a high-resolution wheel splitting one notch into many
  // events) adds up like pixel motion instead of one step per event.
  const steps = deltaY / WHEEL_LINES_PER_SCROLL
  return Math.abs(deltaY) < 1
    ? steps
    : Math.sign(steps) * Math.max(1, Math.abs(steps))
}

export type TerminalScrollDirection = TerminalScrollMessage['direction']

/** One queued `terminal.scroll`: a wheel step or a PageUp/PageDown press. */
export interface ScrollCommand {
  source: 'wheel' | 'page_key'
  direction: TerminalScrollDirection
}

interface ScrollBatchInFlight {
  sentAt: number
  // The newest frame sequence the browser had when the batch was sent. Only
  // a newer frame can answer it.
  afterSeq: number
  // The `terminal.scroll` messages sent on this connection once the batch
  // was sent. A frame Yard tags with at least this many was read after the
  // whole batch reached Herdr.
  scrolls: number
  // Latency of a newer untagged frame that arrived too soon to be the answer.
  earlyFrameMs: number | null
}

/** The frame fields that can answer a scroll batch. */
export interface ScrollAnswerFrame {
  seq: number
  scrolls?: number
  /** Bytes of the frame, for the window's byte budget. */
  bytes?: number
}

/**
 * Scroll flow control: the pending input and the batches in flight.
 *
 * Yard tags every frame with the scroll messages it had forwarded to Herdr
 * when it read the frame, so a batch is answered by the first frame read
 * after the batch reached Herdr, or by the timeout. A frame that was already
 * on its way (Herdr's repaint of the previous batch, a spinner) carries an
 * older count and never releases the next batch early, whatever its timing.
 * The count is cumulative, so one frame answers every batch it covers.
 * Tagged answers measure the round trip, which sizes the window
 * (`scrollBatchWindow`); until then one batch is in flight at a time.
 *
 * An untagged frame (a server without tags) falls back to timing, one batch
 * at a time: it answers when it arrives at least half the first answer's
 * latency after the send. Accepted answers never lower that guard, because
 * a stale frame that slips through would otherwise ratchet it down until
 * every frame answers.
 */
export interface ScrollGate {
  /** Pending wheel steps, signed (negative = up), possibly fractional. */
  wheel: number
  /** Pending PageUp (negative) or PageDown (positive) presses. */
  pages: number
  /**
   * The pending steps are a "Jump to latest" return (queueScrollReturn),
   * not user input: input the same way adds to them without the cap.
   */
  returning: boolean
  /** Batches sent and not answered yet, oldest first. */
  inFlight: ScrollBatchInFlight[]
  /** When the newest batch was sent, for the send interval. */
  lastSentAt: number | null
  /** The untagged-answer guard (see above). */
  fastestAnswerMs: number | null
  /**
   * Fastest of the recent tagged answers (`recentRoundTrips`): the round
   * trip that sizes the window. Recent, not all-time, so a path that slows
   * down (jitter, congestion, Wi-Fi) widens the window again.
   */
  roundTripMs: number | null
  /** The latest tagged answer latencies, oldest first, at most ROUND_TRIP_SAMPLES. */
  recentRoundTrips: number[]
  /** Size of the latest tagged answer frame. */
  answerBytes: number | null
}

export function createScrollGate(): ScrollGate {
  return {
    wheel: 0,
    pages: 0,
    returning: false,
    inFlight: [],
    lastSentAt: null,
    fastestAnswerMs: null,
    roundTripMs: null,
    recentRoundTrips: [],
    answerBytes: null,
  }
}

/**
 * Batches that may be in flight: one per send interval of the measured
 * round trip, plus one so a batch the app does not repaint for never holds
 * the next, within the answer byte budget. One until a tagged answer.
 */
export function scrollBatchWindow(gate: ScrollGate) {
  if (gate.roundTripMs === null) return 1
  const byTime = Math.ceil(gate.roundTripMs / SCROLL_SEND_INTERVAL_MS) + 1
  const byBytes = gate.answerBytes
    ? Math.floor(MAX_SCROLL_ANSWER_BYTES_IN_FLIGHT / gate.answerBytes)
    : MAX_SCROLL_BATCHES_IN_FLIGHT
  return Math.max(
    1,
    Math.min(MAX_SCROLL_BATCHES_IN_FLIGHT, byTime, byBytes),
  )
}

/**
 * Milliseconds until the next batch may leave, or null while the window is
 * full (an answer or the timeout reopens it).
 */
export function scrollSendDelay(gate: ScrollGate, now: number) {
  if (gate.inFlight.length >= scrollBatchWindow(gate)) return null
  if (gate.inFlight.length === 0 || gate.lastSentAt === null) return 0
  return Math.max(0, gate.lastSentAt + SCROLL_SEND_INTERVAL_MS - now)
}

function clampPendingSteps(steps: number) {
  return Math.min(
    MAX_PENDING_SCROLL_STEPS,
    Math.max(-MAX_PENDING_SCROLL_STEPS, steps),
  )
}

function pendingScrollSign(gate: ScrollGate) {
  return Math.sign(gate.pages) || Math.sign(gate.wheel)
}

/**
 * Add wheel steps to the pending input. Input in the other direction
 * cancels whatever is still pending (including a partial step), and the
 * pending total is capped so a long fling cannot queue a backlog. Input the
 * same way as a pending return adds to it uncapped, so the rest of the
 * return is not lost.
 */
export function addWheelSteps(gate: ScrollGate, steps: number): ScrollGate {
  if (!Number.isFinite(steps) || steps === 0) return gate
  const reversed = pendingScrollSign(gate) === -Math.sign(steps)
  const returning = gate.returning && !reversed
  const wheel = (reversed ? 0 : gate.wheel) + steps
  return {
    ...gate,
    wheel: returning ? wheel : clampPendingSteps(wheel),
    pages: reversed ? 0 : gate.pages,
    returning,
  }
}

/** Queue one PageUp/PageDown press, with the same reversal and cap rules. */
export function addPageScroll(
  gate: ScrollGate,
  direction: TerminalScrollDirection,
): ScrollGate {
  const sign = direction === 'up' ? -1 : 1
  const reversed = pendingScrollSign(gate) === -sign
  const returning = gate.returning && !reversed
  const pages = (reversed ? 0 : gate.pages) + sign
  return {
    ...gate,
    wheel: reversed ? 0 : gate.wheel,
    pages: returning ? pages : clampPendingSteps(pages),
    returning,
  }
}

/**
 * Take the next batch when the window has room and the send interval has
 * passed (at once when nothing is in flight): queued page presses first,
 * then whole wheel steps, at most `MAX_SCROLL_BATCH_COMMANDS` commands. The
 * fractional step stays pending. `lastSeq` is the newest frame sequence the
 * browser has seen.
 */
export function takeScrollBatch(
  gate: ScrollGate,
  now: number,
  lastSeq: number,
  scrollsSent: number,
): { gate: ScrollGate; commands: ScrollCommand[] } {
  if (scrollSendDelay(gate, now) !== 0) return { gate, commands: [] }
  const commands: ScrollCommand[] = []
  let { pages, wheel } = gate
  while (pages !== 0 && commands.length < MAX_SCROLL_BATCH_COMMANDS) {
    commands.push({ source: 'page_key', direction: pages < 0 ? 'up' : 'down' })
    pages -= Math.sign(pages)
  }
  const sign = Math.sign(wheel)
  const whole = Math.trunc(Math.abs(wheel) + WHEEL_STEP_EPSILON)
  const steps = Math.min(whole, MAX_SCROLL_BATCH_COMMANDS - commands.length)
  for (let step = 0; step < steps; step += 1) {
    commands.push({ source: 'wheel', direction: sign < 0 ? 'up' : 'down' })
  }
  wheel -= sign * steps
  if (Math.abs(wheel) < WHEEL_STEP_EPSILON) wheel = 0
  if (commands.length === 0) return { gate, commands }
  return {
    gate: {
      ...gate,
      pages,
      wheel,
      // A return ends once no whole command of it is left.
      returning:
        gate.returning &&
        (pages !== 0 || Math.abs(wheel) + WHEEL_STEP_EPSILON >= 1),
      inFlight: [
        ...gate.inFlight,
        {
          sentAt: now,
          afterSeq: lastSeq,
          scrolls: scrollsSent + groupScrollCommands(commands).length,
          earlyFrameMs: null,
        },
      ],
      lastSentAt: now,
    },
    commands,
  }
}

/** Record a frame the browser has written; it may answer batches. */
export function answerScrollBatch(
  gate: ScrollGate,
  frame: ScrollAnswerFrame,
  now: number,
): ScrollGate {
  const flights = gate.inFlight
  if (flights.length === 0) return gate
  if (typeof frame.scrolls === 'number') {
    const tag = frame.scrolls
    const open = flights.filter(
      (flight) => frame.seq <= flight.afterSeq || tag < flight.scrolls,
    )
    if (open.length === flights.length) return gate
    const answered = flights.filter((flight) => !open.includes(flight))
    const latency = Math.max(0, now - answered[answered.length - 1].sentAt)
    const recentRoundTrips = [...gate.recentRoundTrips, latency].slice(
      -ROUND_TRIP_SAMPLES,
    )
    return {
      ...gate,
      inFlight: open,
      roundTripMs: Math.min(...recentRoundTrips),
      recentRoundTrips,
      answerBytes: frame.bytes ?? gate.answerBytes,
    }
  }
  // Untagged frames: one batch at a time (the window stays at one).
  const flight = flights[0]
  if (frame.seq <= flight.afterSeq) return gate
  const latency = Math.max(0, now - flight.sentAt)
  if (gate.fastestAnswerMs !== null && latency < gate.fastestAnswerMs / 2) {
    if (flight.earlyFrameMs !== null) return gate
    return {
      ...gate,
      inFlight: [{ ...flight, earlyFrameMs: latency }, ...flights.slice(1)],
    }
  }
  return {
    ...gate,
    inFlight: flights.slice(1),
    fastestAnswerMs: gate.fastestAnswerMs ?? latency,
  }
}

/** When the oldest batch in flight times out, or null if none is. */
export function scrollBatchDeadline(gate: ScrollGate) {
  const oldest = gate.inFlight[0]
  return oldest ? oldest.sentAt + SCROLL_BATCH_TIMEOUT_MS : null
}

/**
 * Batches sent at least `SCROLL_BATCH_TIMEOUT_MS` before `now` (all of them
 * by default) timed out. An early untagged frame seen meanwhile means
 * answers now come faster than the guard allows, so it lowers the guard
 * instead of letting every later batch wait for the timeout too, but never
 * below `MIN_UNTAGGED_ANSWER_ESTIMATE_MS`: a batch that got no answer (top
 * of the scrollback) must not let Herdr's second render of the previous
 * batch release every later batch.
 */
export function expireScrollBatch(
  gate: ScrollGate,
  now = Number.POSITIVE_INFINITY,
): ScrollGate {
  const expired = gate.inFlight.filter(
    (flight) => flight.sentAt + SCROLL_BATCH_TIMEOUT_MS <= now,
  )
  if (expired.length === 0) return gate
  let { fastestAnswerMs } = gate
  for (const { earlyFrameMs } of expired) {
    if (earlyFrameMs === null) continue
    fastestAnswerMs = Math.min(
      fastestAnswerMs ?? earlyFrameMs,
      Math.max(earlyFrameMs, MIN_UNTAGGED_ANSWER_ESTIMATE_MS),
    )
  }
  return {
    ...gate,
    inFlight: gate.inFlight.filter((flight) => !expired.includes(flight)),
    fastestAnswerMs,
  }
}

/**
 * Input or a scroll reset returns Herdr to the latest output, so pending
 * scrolls are dropped and the next one is sent at once. The latency guard,
 * round trip, and answer size are properties of the connection and are kept.
 */
export function clearScrollGate(gate: ScrollGate): ScrollGate {
  return {
    ...createScrollGate(),
    fastestAnswerMs: gate.fastestAnswerMs,
    roundTripMs: gate.roundTripMs,
    recentRoundTrips: gate.recentRoundTrips,
    answerBytes: gate.answerBytes,
  }
}

/** Zero-based cell under the pointer, for wheel reports to mouse-aware apps. */
export function pointerTerminalCell(
  clientX: number,
  clientY: number,
  screen: { left: number; top: number; width: number; height: number },
  size: TerminalSize,
): TerminalCell {
  const column =
    screen.width > 0
      ? Math.floor(((clientX - screen.left) / screen.width) * size.cols)
      : 0
  const row =
    screen.height > 0
      ? Math.floor(((clientY - screen.top) / screen.height) * size.rows)
      : 0
  return {
    column: Math.min(size.cols - 1, Math.max(0, column)),
    row: Math.min(size.rows - 1, Math.max(0, row)),
  }
}

export function wheelScrollMessage(
  lines: number,
  cell: TerminalCell | null,
): TerminalScrollMessage {
  return {
    type: 'terminal.scroll',
    direction: lines < 0 ? 'up' : 'down',
    lines: Math.abs(lines),
    source: 'wheel',
    ...(cell ?? {}),
  }
}

/**
 * PageUp/PageDown, plain or with Shift (xterm's scrollback convention), are
 * sent to Herdr as page-key scrolls. Herdr scrolls its own scrollback at a
 * shell-like prompt and otherwise forwards PageUp/PageDown to the app, like
 * its own attach client. Ctrl, Alt, and Meta variants stay ordinary input.
 */
export function pageScrollMessage(
  event: Pick<KeyboardEvent, 'altKey' | 'ctrlKey' | 'key' | 'metaKey'>,
  viewportRows: number,
): TerminalScrollMessage | null {
  if (event.altKey || event.ctrlKey || event.metaKey) return null
  const direction =
    event.key === 'PageUp' ? 'up' : event.key === 'PageDown' ? 'down' : null
  if (!direction) return null
  return {
    type: 'terminal.scroll',
    direction,
    lines: pageScrollLines(viewportRows),
    source: 'page_key',
  }
}

export function pageScrollLines(viewportRows: number) {
  return Math.min(MAX_TERMINAL_ROWS, Math.max(1, Math.floor(viewportRows) - 1))
}

/**
 * The `terminal.scroll` for one batched command: a wheel step is one
 * notch-sized command at the pointer cell, a page press scrolls one screen.
 */
export function scrollCommandMessage(
  command: ScrollCommand,
  cell: TerminalCell | null,
  viewportRows: number,
): TerminalScrollMessage {
  if (command.source === 'page_key') {
    return {
      type: 'terminal.scroll',
      direction: command.direction,
      lines: pageScrollLines(viewportRows),
      source: 'page_key',
    }
  }
  return wheelScrollMessage(
    (command.direction === 'up' ? -1 : 1) * WHEEL_LINES_PER_SCROLL,
    cell,
  )
}

function groupScrollCommands(commands: ScrollCommand[]) {
  const groups: { command: ScrollCommand; count: number }[] = []
  for (const command of commands) {
    const last = groups.at(-1)
    if (
      last &&
      last.command.source === command.source &&
      last.command.direction === command.direction
    ) {
      last.count += 1
    } else {
      groups.push({ command, count: 1 })
    }
  }
  return groups
}

/**
 * The messages for one batch: each run of identical commands becomes one
 * `terminal.scroll` with a `count`. Yard writes the run to Herdr in one go,
 * so Herdr answers the batch with one repaint (not one at once and another
 * a render interval later), yet a mouse-aware app still gets one wheel
 * report per step.
 */
export function scrollBatchMessages(
  commands: ScrollCommand[],
  cell: TerminalCell | null,
  viewportRows: number,
): TerminalScrollMessage[] {
  return groupScrollCommands(commands).map(({ command, count }) => {
    const message = scrollCommandMessage(command, cell, viewportRows)
    return count > 1 ? { ...message, count } : message
  })
}

/** Most commands "Jump to latest" queues to undo a view's own scrolling. */
export const MAX_RETURN_SCROLL_COMMANDS = 1000

/** Net scroll commands a view has sent up (never below zero). */
export interface ScrolledBackCommands {
  wheel: number
  pages: number
}

export function scrolledBackCommands(
  net: ScrolledBackCommands,
  message: Pick<TerminalScrollMessage, 'count' | 'direction' | 'source'>,
): ScrolledBackCommands {
  const delta = (message.direction === 'up' ? 1 : -1) * (message.count ?? 1)
  return message.source === 'page_key'
    ? { ...net, pages: Math.max(0, net.pages + delta) }
    : { ...net, wheel: Math.max(0, net.wheel + delta) }
}

/**
 * Queue the scrolls that return a view to where it was before it scrolled
 * back, after a `terminal.scroll_reset`.
 *
 * The reset only moves Herdr's own scrollback. A full-screen app (Claude
 * Code, Codex: alternate screen with mouse reporting) owns its transcript
 * scroll, and Herdr turned each wheel command into one wheel report and each
 * page command into one PageUp, so the reset leaves the app scrolled back.
 * The same number of commands go back down, paced by the gate like the
 * scrolls that went up (an app may coalesce a burst of wheel reports). For
 * Herdr's scrollback the reset is already at the bottom, where they do
 * nothing. Unlike user input this is not capped at
 * `MAX_PENDING_SCROLL_STEPS` (nor is input the same way added to it); input
 * the other way still cancels it.
 */
export function queueScrollReturn(
  gate: ScrollGate,
  net: ScrolledBackCommands,
): ScrollGate {
  const pages = Math.min(
    Math.max(0, Math.trunc(net.pages)),
    MAX_RETURN_SCROLL_COMMANDS,
  )
  const wheel = Math.min(
    Math.max(0, Math.trunc(net.wheel)),
    MAX_RETURN_SCROLL_COMMANDS - pages,
  )
  return {
    ...clearScrollGate(gate),
    pages,
    wheel,
    returning: pages + wheel > 0,
  }
}

/**
 * Whether the user opted in to xterm's screen-reader mode (for example
 * `localStorage.setItem('yard:terminal-screen-reader:v1', 'on')`). Read when
 * a terminal opens.
 */
export function readTerminalScreenReaderPreference(
  storage: Pick<Storage, 'getItem'> | null | undefined,
) {
  let value: string | null = null
  try {
    value = storage?.getItem(TERMINAL_SCREEN_READER_STORAGE_KEY) ?? null
  } catch {
    return false
  }
  return value === 'on' || value === 'true' || value === '1'
}

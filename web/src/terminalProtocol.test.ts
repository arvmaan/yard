import { describe, expect, it } from 'vitest'
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
  MAX_SCROLL_BATCH_COMMANDS,
  MIN_UNTAGGED_ANSWER_ESTIMATE_MS,
  MAX_TERMINAL_COLS,
  MAX_TERMINAL_PASTE_BYTES,
  MAX_TERMINAL_ROWS,
  MIN_TERMINAL_COLS,
  MIN_TERMINAL_ROWS,
  pageScrollMessage,
  pointerTerminalCell,
  readTerminalScreenReaderPreference,
  type ScrollGate,
  scrollBatchMessages,
  scrollCommandMessage,
  takeScrollBatch,
  TERMINAL_SCREEN_READER_STORAGE_KEY,
  WHEEL_PIXELS_PER_STEP,
  wheelEventSteps,
  wheelScrollMessage,
  wrapHerdrFrame,
} from './terminalProtocol'

const decoder = new TextDecoder()

describe('clampTerminalSize', () => {
  it('keeps a fitted size inside the range the server accepts', () => {
    expect(clampTerminalSize({ cols: 470, rows: 40 })).toEqual({
      cols: MAX_TERMINAL_COLS,
      rows: 40,
    })
    expect(clampTerminalSize({ cols: 142, rows: 260 })).toEqual({
      cols: 142,
      rows: MAX_TERMINAL_ROWS,
    })
    expect(clampTerminalSize({ cols: 2, rows: 1 })).toEqual({
      cols: MIN_TERMINAL_COLS,
      rows: MIN_TERMINAL_ROWS,
    })
    expect(clampTerminalSize({ cols: 80.9, rows: 24.2 })).toEqual({
      cols: 80,
      rows: 24,
    })
    expect(clampTerminalSize({ cols: Number.NaN, rows: 24 })).toEqual({
      cols: MIN_TERMINAL_COLS,
      rows: 24,
    })
  })
})

describe('wrapHerdrFrame', () => {
  it('writes each Herdr frame with autowrap disabled and restores it', () => {
    const frame = new TextEncoder().encode('\u001b[1;1HR01')
    const wrapped = wrapHerdrFrame(frame)

    expect(decoder.decode(wrapped)).toBe('\u001b[?7l\u001b[1;1HR01\u001b[?7h')
    expect(decoder.decode(wrapHerdrFrame(new Uint8Array()))).toBe(
      '\u001b[?7l\u001b[?7h',
    )
  })
})

describe('bracketedTerminalPaste', () => {
  it('sends a multi-line paste as one bracketed paste with terminal newlines', () => {
    expect(bracketedTerminalPaste('first\nsecond\r\nthird')).toEqual({
      kind: 'ready',
      text: '\u001b[200~first\rsecond\rthird\u001b[201~',
      bytes: 30,
    })
  })

  it('removes embedded paste markers so pasted text cannot end the paste', () => {
    const paste = bracketedTerminalPaste(
      'safe\u001b[201~rm -rf /\u001b[200~tail',
    )
    expect(paste).toMatchObject({
      kind: 'ready',
      text: '\u001b[200~saferm -rf /tail\u001b[201~',
    })
  })

  it('replaces lone surrogates the server could not decode', () => {
    expect(bracketedTerminalPaste('a\uD800b🚀')).toMatchObject({
      kind: 'ready',
      text: '\u001b[200~a\uFFFDb🚀\u001b[201~',
    })
  })

  it('counts UTF-8 bytes and rejects a paste above the limit', () => {
    const atLimit = 'x'.repeat(MAX_TERMINAL_PASTE_BYTES - 12)
    expect(bracketedTerminalPaste(atLimit)).toMatchObject({
      kind: 'ready',
      bytes: MAX_TERMINAL_PASTE_BYTES,
    })
    expect(bracketedTerminalPaste(`${atLimit}é`)).toEqual({
      kind: 'too_large',
      bytes: MAX_TERMINAL_PASTE_BYTES + 2,
      limit: MAX_TERMINAL_PASTE_BYTES,
    })
  })

  it('ignores an empty paste', () => {
    expect(bracketedTerminalPaste('')).toBeNull()
    expect(bracketedTerminalPaste('\u001b[200~\u001b[201~')).toBeNull()
  })

  it('formats byte sizes for the paste notice', () => {
    expect(formatTerminalBytes(900)).toBe('900 B')
    expect(formatTerminalBytes(512 * 1024)).toBe('512 KiB')
    expect(formatTerminalBytes(3 * 1024 * 1024)).toBe('3.0 MiB')
  })
})

describe('wheel sensitivity', () => {
  it('counts one step per ~44 px of pixel-mode wheel motion', () => {
    expect(WHEEL_PIXELS_PER_STEP).toBe(44)
    expect(wheelEventSteps(-44, 0, 40)).toBe(-1)
    expect(wheelEventSteps(132, 0, 40)).toBe(3)
    expect(wheelEventSteps(-2, 0, 40)).toBeCloseTo(-2 / 44)
    expect(wheelEventSteps(0, 0, 40)).toBe(0)
    expect(wheelEventSteps(Number.NaN, 0, 40)).toBe(0)
  })

  it('moves at least one step for every discrete line or page wheel event', () => {
    // Firefox reports a mouse notch as 3 lines; a one-line notch still
    // moves one step.
    expect(wheelEventSteps(3, 1, 40)).toBe(1)
    expect(wheelEventSteps(-1, 1, 40)).toBe(-1)
    expect(wheelEventSteps(6, 1, 40)).toBe(2)
    expect(wheelEventSteps(-1, 2, 40)).toBe(-40 / 3)
    expect(wheelEventSteps(0.01, 2, 40)).toBe(1)
  })

  it('adds up sub-line line-mode deltas like pixel motion', () => {
    // A high-resolution wheel reporting one 3-line notch as 8 events of
    // 0.375 lines is one step, not eight.
    let gate = createScrollGate()
    for (let event = 0; event < 8; event += 1) {
      gate = addWheelSteps(gate, wheelEventSteps(0.375, 1, 40))
    }
    expect(gate.wheel).toBeCloseTo(1)
    expect(takeScrollBatch(gate, 0, 0, 0).commands).toEqual([
      { source: 'wheel', direction: 'down' },
    ])
    expect(wheelEventSteps(-0.25, 1, 40)).toBeCloseTo(-0.25 / 3)
  })

  it('adds up a slow trackpad drag instead of dropping it between events', () => {
    // 2 px per event is a slow drag: 22 events make one step, however long
    // the pauses between them were.
    let gate = createScrollGate()
    for (let i = 0; i < 21; i += 1) {
      gate = addWheelSteps(gate, wheelEventSteps(-2, 0, 40))
    }
    expect(takeScrollBatch(gate, 0, 0, 0).commands).toEqual([])
    gate = addWheelSteps(gate, wheelEventSteps(-2, 0, 40))
    expect(takeScrollBatch(gate, 0, 0, 0).commands).toEqual([
      { source: 'wheel', direction: 'up' },
    ])
  })

  it('counts whole steps despite float drift in summed pixel deltas', () => {
    let gate = createScrollGate()
    for (let i = 0; i < 11; i += 1) {
      gate = addWheelSteps(gate, wheelEventSteps(-4, 0, 40))
    }
    const batch = takeScrollBatch(gate, 0, 0, 0)
    expect(batch.commands).toHaveLength(1)
    expect(batch.gate.wheel).toBe(0)
  })
})

describe('scroll flow control', () => {
  const up = { source: 'wheel', direction: 'up' } as const
  const down = { source: 'wheel', direction: 'down' } as const

  const sent = (
    gate: ScrollGate,
    now: number,
    lastSeq: number,
    scrollsSent = 0,
  ) => {
    const batch = takeScrollBatch(gate, now, lastSeq, scrollsSent)
    return { gate: batch.gate, commands: batch.commands }
  }

  it('sends the first batch at once, then holds input until an answer', () => {
    let gate = addWheelSteps(createScrollGate(), -2)
    let batch = sent(gate, 1_000, 7)
    expect(batch.commands).toEqual([up, up])
    gate = batch.gate
    expect(gate.inFlight).toMatchObject({ sentAt: 1_000, afterSeq: 7 })

    // More input while the batch is in flight is summed, not sent.
    gate = addWheelSteps(gate, -1.5)
    gate = addWheelSteps(gate, -1)
    expect(sent(gate, 1_010, 7).commands).toEqual([])

    // A frame the browser already had (or an older one) is not the answer.
    gate = answerScrollBatch(gate, { seq: 7 }, 1_130)
    expect(gate.inFlight).not.toBeNull()

    // The next newer frame is; the summed input then goes as one batch and
    // the half step stays pending.
    gate = answerScrollBatch(gate, { seq: 8 }, 1_136)
    expect(gate.inFlight).toBeNull()
    expect(gate.fastestAnswerMs).toBe(136)
    batch = sent(gate, 1_137, 8)
    expect(batch.commands).toEqual([up, up])
    expect(batch.gate.wheel).toBeCloseTo(-0.5)
    expect(batch.gate.inFlight).toMatchObject({ afterSeq: 8 })
  })

  it('caps a batch at five commands and the pending input at five steps', () => {
    let gate = addWheelSteps(createScrollGate(), -9.1)
    expect(gate.wheel).toBe(-5)
    const batch = sent(gate, 0, 0)
    expect(batch.commands).toHaveLength(MAX_SCROLL_BATCH_COMMANDS)
    expect(batch.gate.wheel).toBe(0)
    gate = batch.gate
    for (let i = 0; i < 40; i += 1) gate = addWheelSteps(gate, -1)
    expect(gate.wheel).toBe(-5)
    for (let i = 0; i < 8; i += 1) gate = addPageScroll(gate, 'up')
    expect(gate.pages).toBe(-5)
    gate = answerScrollBatch(gate, { seq: 1 }, 150)
    // Page presses go first; a batch never exceeds five commands.
    const next = sent(gate, 150, 1)
    expect(next.commands).toEqual(
      Array.from({ length: 5 }, () => ({
        source: 'page_key',
        direction: 'up',
      })),
    )
    expect(next.gate).toMatchObject({ pages: 0, wheel: -5 })
  })

  it('cancels pending input in the other direction, including a partial step', () => {
    let gate = sent(addWheelSteps(createScrollGate(), -1), 0, 0).gate
    gate = addWheelSteps(gate, -3.5)
    gate = addPageScroll(gate, 'up')
    gate = addWheelSteps(gate, 1.25)
    expect(gate).toMatchObject({ wheel: 1.25, pages: 0 })
    gate = addPageScroll(gate, 'up')
    expect(gate).toMatchObject({ wheel: 0, pages: -1 })
    // A leftover fraction never eats into the next opposite step.
    gate = clearScrollGate(addWheelSteps(createScrollGate(), -0.9))
    expect(gate.wheel).toBe(0)
    gate = addWheelSteps(addWheelSteps(createScrollGate(), -0.9), 1)
    expect(sent(gate, 0, 0).commands).toEqual([down])
  })

  it('answers a batch only with a frame Yard read after the batch reached Herdr', () => {
    // Two wheel steps are one counted message; a page press and a wheel
    // step are two messages.
    let gate = sent(addWheelSteps(createScrollGate(), -2), 1_000, 7, 4).gate
    expect(gate.inFlight).toMatchObject({ afterSeq: 7, scrolls: 5 })
    // Herdr's repaint of an earlier batch is newer, and late enough to pass
    // any timing guard, but Yard read it before this batch arrived.
    gate = answerScrollBatch(gate, { seq: 8, scrolls: 4 }, 1_200)
    expect(gate.inFlight).not.toBeNull()
    // The first frame read after the batch answers it, however soon.
    gate = answerScrollBatch(gate, { seq: 9, scrolls: 5 }, 1_001)
    expect(gate.inFlight).toBeNull()
    expect(gate.fastestAnswerMs).toBeNull()

    gate = addWheelSteps(addPageScroll(gate, 'up'), -1)
    gate = sent(gate, 2_000, 9, 5).gate
    expect(gate.inFlight).toMatchObject({ scrolls: 7 })
    gate = answerScrollBatch(gate, { seq: 10, scrolls: 6 }, 2_130)
    expect(gate.inFlight).not.toBeNull()
    expect(
      answerScrollBatch(gate, { seq: 11, scrolls: 7 }, 2_140).inFlight,
    ).toBeNull()
  })

  it('never lets stale tagged or untagged frames ratchet the gate open', () => {
    // A real tunnel: every true answer takes a round trip (125 ms). Each
    // batch first sees a stale frame (the previous batch's second repaint)
    // at 0.6x the previous stale latency, down to one frame every 17 ms.
    const stale = [75, 45, 27, 16, 17, 17]
    for (const tagged of [false, true]) {
      let seq = 1
      const frame = (scrolls: number) => ({
        seq: (seq += 1),
        ...(tagged ? { scrolls } : {}),
      })
      let gate = sent(addWheelSteps(createScrollGate(), -1), 0, seq, 0).gate
      gate = answerScrollBatch(gate, frame(1), 125)
      expect(gate.inFlight).toBeNull()
      let answeredEarly = 0
      stale.forEach((latency, index) => {
        const sentAt = 200 * (index + 1)
        const scrolls = index + 1
        gate = sent(addWheelSteps(gate, -1), sentAt, seq, scrolls).gate
        // Tagged, the stale frame still carries the previous batch's count.
        gate = answerScrollBatch(gate, frame(scrolls), sentAt + latency)
        if (!gate.inFlight) {
          answeredEarly += 1
          return
        }
        gate = answerScrollBatch(gate, frame(scrolls + 1), sentAt + 125)
        expect(gate.inFlight).toBeNull()
      })
      // Untagged, the 75 ms frame passes the 62.5 ms guard once; accepted
      // answers never lower the guard, so 45, 27, 16, and 17 ms never do.
      expect(answeredEarly).toBe(tagged ? 0 : 1)
      if (!tagged) expect(gate.fastestAnswerMs).toBe(125)
    }
  })

  it('ignores a frame that arrives sooner than half the fastest answer', () => {
    let gate = sent(addWheelSteps(createScrollGate(), -1), 0, 0).gate
    gate = answerScrollBatch(gate, { seq: 1 }, 130)
    expect(gate.fastestAnswerMs).toBe(130)
    gate = sent(addWheelSteps(gate, -1), 200, 1).gate
    // Herdr's second render of the previous batch, 17 ms later.
    gate = answerScrollBatch(gate, { seq: 2 }, 217)
    expect(gate.inFlight).toMatchObject({ earlyFrameMs: 17 })
    gate = answerScrollBatch(gate, { seq: 3 }, 332)
    expect(gate.inFlight).toBeNull()
    expect(gate.fastestAnswerMs).toBe(130)
  })

  it('treats a batch without a frame as answered at the timeout', () => {
    let gate = sent(addWheelSteps(createScrollGate(), -1), 0, 0).gate
    gate = addWheelSteps(gate, -2)
    gate = expireScrollBatch(gate)
    expect(gate.inFlight).toBeNull()
    expect(gate.fastestAnswerMs).toBeNull()
    expect(sent(gate, 300, 0).commands).toEqual([up, up])
    expect(expireScrollBatch(createScrollGate())).toEqual(createScrollGate())
  })

  it('lowers the guard when only an early frame came before the timeout', () => {
    let gate = sent(addWheelSteps(createScrollGate(), -1), 0, 0).gate
    gate = answerScrollBatch(gate, { seq: 1 }, 300)
    gate = sent(addWheelSteps(gate, -1), 400, 1).gate
    gate = answerScrollBatch(gate, { seq: 2 }, 460)
    expect(gate.inFlight).not.toBeNull()
    gate = expireScrollBatch(gate)
    expect(gate.fastestAnswerMs).toBe(60)
    gate = sent(addWheelSteps(gate, -1), 800, 2).gate
    gate = answerScrollBatch(gate, { seq: 3 }, 860)
    expect(gate.inFlight).toBeNull()
  })

  it("never lowers the untagged guard below Herdr's second render after a timeout", () => {
    // A 126 ms answer, then a batch at the top of the scrollback: only the
    // previous batch's second render (17 ms) arrives before the timeout.
    let seq = 1
    let gate = sent(addWheelSteps(createScrollGate(), -1), 0, seq).gate
    gate = answerScrollBatch(gate, { seq: (seq += 1) }, 126)
    gate = sent(addWheelSteps(gate, -1), 126, seq).gate
    gate = answerScrollBatch(gate, { seq: (seq += 1) }, 143)
    expect(gate.inFlight).toMatchObject({ earlyFrameMs: 17 })
    gate = expireScrollBatch(gate)
    expect(gate.fastestAnswerMs).toBe(MIN_UNTAGGED_ANSWER_ESTIMATE_MS)
    // Later second renders 17 ms after each send never release a batch.
    for (let batch = 0; batch < 10; batch += 1) {
      const sentAt = 1_000 + batch * 200
      gate = sent(addWheelSteps(gate, -1), sentAt, seq).gate
      gate = answerScrollBatch(gate, { seq: (seq += 1) }, sentAt + 17)
      expect(gate.inFlight).not.toBeNull()
      gate = answerScrollBatch(gate, { seq: (seq += 1) }, sentAt + 126)
      expect(gate.inFlight).toBeNull()
    }
  })

  it('drops pending input and the in-flight batch on input, keeping the guard', () => {
    let gate = sent(addWheelSteps(createScrollGate(), -1), 0, 0).gate
    gate = answerScrollBatch(gate, { seq: 1 }, 120)
    gate = sent(addWheelSteps(gate, -1), 200, 1).gate
    gate = addWheelSteps(addPageScroll(gate, 'up'), -2)
    expect(clearScrollGate(gate)).toEqual({
      ...createScrollGate(),
      fastestAnswerMs: 120,
    })
  })

  it('sends each run of identical commands as one counted message', () => {
    const cell = { column: 1, row: 2 }
    const page = { source: 'page_key', direction: 'up' } as const
    expect(scrollBatchMessages([page, up, up, up, up], cell, 40)).toEqual([
      {
        type: 'terminal.scroll',
        direction: 'up',
        lines: 39,
        source: 'page_key',
      },
      {
        type: 'terminal.scroll',
        direction: 'up',
        lines: 3,
        source: 'wheel',
        column: 1,
        row: 2,
        count: 4,
      },
    ])
    expect(scrollBatchMessages([down], null, 40)).toEqual([
      { type: 'terminal.scroll', direction: 'down', lines: 3, source: 'wheel' },
    ])
    expect(scrollBatchMessages([], null, 40)).toEqual([])
  })

  it('turns batched commands into Herdr terminal.scroll messages', () => {
    const cell = { column: 1, row: 2 }
    expect(scrollCommandMessage(up, cell, 40)).toEqual({
      type: 'terminal.scroll',
      direction: 'up',
      lines: 3,
      source: 'wheel',
      column: 1,
      row: 2,
    })
    expect(scrollCommandMessage(down, null, 40)).toEqual({
      type: 'terminal.scroll',
      direction: 'down',
      lines: 3,
      source: 'wheel',
    })
    expect(
      scrollCommandMessage({ source: 'page_key', direction: 'up' }, cell, 40),
    ).toEqual({
      type: 'terminal.scroll',
      direction: 'up',
      lines: 39,
      source: 'page_key',
    })
  })
})

describe('wheel scrolling', () => {
  it('reports the zero-based cell under the pointer', () => {
    const screen = { left: 10, top: 20, width: 800, height: 480 }
    const size = { cols: 100, rows: 40 }
    expect(pointerTerminalCell(10, 20, screen, size)).toEqual({
      column: 0,
      row: 0,
    })
    expect(pointerTerminalCell(418, 263, screen, size)).toEqual({
      column: 51,
      row: 20,
    })
    expect(pointerTerminalCell(5000, -5, screen, size)).toEqual({
      column: 99,
      row: 0,
    })
  })

  it('builds Herdr terminal.scroll wheel commands', () => {
    expect(wheelScrollMessage(-3, { column: 4, row: 7 })).toEqual({
      type: 'terminal.scroll',
      direction: 'up',
      lines: 3,
      source: 'wheel',
      column: 4,
      row: 7,
    })
    expect(wheelScrollMessage(2, null)).toEqual({
      type: 'terminal.scroll',
      direction: 'down',
      lines: 2,
      source: 'wheel',
    })
  })
})

describe('readTerminalScreenReaderPreference', () => {
  const storage = (value: string | null) => ({
    getItem: (key: string) =>
      key === TERMINAL_SCREEN_READER_STORAGE_KEY ? value : null,
  })

  it('keeps screen-reader mode off unless the user opted in', () => {
    expect(TERMINAL_SCREEN_READER_STORAGE_KEY).toBe(
      'yard:terminal-screen-reader:v1',
    )
    expect(readTerminalScreenReaderPreference(storage(null))).toBe(false)
    expect(readTerminalScreenReaderPreference(storage('off'))).toBe(false)
    expect(readTerminalScreenReaderPreference(null)).toBe(false)
    expect(
      readTerminalScreenReaderPreference({
        getItem: () => {
          throw new Error('storage disabled')
        },
      }),
    ).toBe(false)
    for (const value of ['on', 'true', '1']) {
      expect(readTerminalScreenReaderPreference(storage(value))).toBe(true)
    }
  })
})

describe('pageScrollMessage', () => {
  const key = (
    name: string,
    modifiers: Partial<Record<'altKey' | 'ctrlKey' | 'metaKey', boolean>> = {},
  ) => ({
    altKey: false,
    ctrlKey: false,
    key: name,
    metaKey: false,
    ...modifiers,
  })

  it('pages through Herdr for plain and Shift PageUp/PageDown', () => {
    expect(pageScrollMessage(key('PageUp'), 40)).toEqual({
      type: 'terminal.scroll',
      direction: 'up',
      lines: 39,
      source: 'page_key',
    })
    expect(pageScrollMessage(key('PageDown'), 40)).toMatchObject({
      direction: 'down',
      lines: 39,
    })
    expect(pageScrollMessage(key('PageUp'), 1)).toMatchObject({ lines: 1 })
  })

  it('leaves other keys and modified page keys as terminal input', () => {
    expect(pageScrollMessage(key('ArrowUp'), 40)).toBeNull()
    expect(pageScrollMessage(key('PageUp', { ctrlKey: true }), 40)).toBeNull()
    expect(pageScrollMessage(key('PageDown', { altKey: true }), 40)).toBeNull()
    expect(pageScrollMessage(key('PageUp', { metaKey: true }), 40)).toBeNull()
  })
})

import { describe, expect, it } from 'vitest'
import {
  HISTORY_ROW_LINES,
  historyEntryLines,
  historyModeEligible,
  historyRowKey,
  historyRows,
  locateRows,
  mergeScreen,
  olderHistorySeam,
  olderLinesStart,
  prependSeam,
  refreshSeam,
  rowKey,
  screenSeam,
  type SerializableCell,
  serializeRow,
  stripTerminalEscapes,
} from './terminalHistory'

const lines = (prefix: string, count: number, from = 1) =>
  Array.from({ length: count }, (_, index) => `${prefix} ${from + index}`)

// The screen Herdr paints for the bottom of `transcript` on a `rows` high
// pane: the last content rows, then blank rows below the cursor.
const screenOf = (transcript: readonly string[], rows: number, blank = 0) => [
  ...transcript.slice(transcript.length - (rows - blank)),
  ...Array.from({ length: blank }, () => ''),
]

const keys = (rows: readonly string[]) => rows.map(rowKey)

// A small deterministic generator for the repeated-content checks.
const random = (seed: number) => () => {
  seed = (seed * 1103515245 + 12345) & 0x7fffffff
  return seed / 0x7fffffff
}

const merge = (history: readonly string[], screen: readonly string[]) => {
  const seam = screenSeam(keys(history), keys(screen))
  return seam === null ? null : mergeScreen(history, seam, screen)
}

const withoutTrailingBlanks = (rows: readonly string[]) => {
  const copy = [...rows]
  while (copy.length > 0 && copy[copy.length - 1] === '') copy.pop()
  return copy
}

describe('historyModeEligible', () => {
  const read = (scroll: {
    offset_from_bottom: number
    max_offset_from_bottom: number
    viewport_rows: number
  } | null, format = 'ansi') => ({ format, text: 'rows', truncated: true, scroll })

  it('applies only when Herdr scrolled its own scrollback for the pane', () => {
    // Measured on an isolated Herdr 0.9.1 (history-mode-notes.md): a shell
    // after one wheel step up.
    expect(
      historyModeEligible(
        read({ offset_from_bottom: 3, max_offset_from_bottom: 2_975, viewport_rows: 30 }),
        30,
      ),
    ).toBe(true)
    // Alternate screen (full-screen Claude Code, less): no scrollback.
    expect(
      historyModeEligible(
        read({ offset_from_bottom: 0, max_offset_from_bottom: 0, viewport_rows: 30 }),
        30,
      ),
    ).toBe(false)
    // An inline app that captures the mouse: Herdr has scrollback but sent
    // the wheel to the app and reset its offset.
    expect(
      historyModeEligible(
        read({ offset_from_bottom: 0, max_offset_from_bottom: 3_027, viewport_rows: 30 }),
        30,
      ),
    ).toBe(false)
  })

  it('stays native without colour rows, a scroll position, or a matching grid', () => {
    const scroll = { offset_from_bottom: 3, max_offset_from_bottom: 90, viewport_rows: 30 }
    expect(historyModeEligible(read(scroll, 'text'), 30)).toBe(false)
    expect(historyModeEligible(read(scroll, 'plain_text'), 30)).toBe(false)
    expect(historyModeEligible(read(null), 30)).toBe(false)
    expect(historyModeEligible({ format: 'ansi', text: '', truncated: false }, 30)).toBe(false)
    expect(historyModeEligible(read(scroll), 29)).toBe(false)
  })
})

describe('history rows and keys', () => {
  it('splits Herdr rendered rows and compares them without styling or spacing', () => {
    const text = '\u001b[0m\u001b[38;5;3mline-2004\u001b[0m abc\r\n\u001b]8;;https://x\u001b\\link\u001b]8;;\u001b\\\r\nbash-4.2$ '
    const rows = historyRows(text)
    expect(rows).toHaveLength(3)
    expect(rows.map(stripTerminalEscapes)).toEqual([
      'line-2004 abc',
      'link',
      'bash-4.2$ ',
    ])
    expect(historyRowKey(rows[0])).toBe('line-2004abc')
    // Herdr moves past a wide character with CUP, so the live screen can
    // hold a padding cell the history row does not.
    expect(rowKey('⏺ Done')).toBe(rowKey('⏺  Done'))
    expect(rowKey('   ')).toBe('')
    expect(historyRows('')).toEqual([])
  })
})

describe('screenSeam', () => {
  const rows = 8

  it('ends the history with the live screen when nothing new arrived', () => {
    const transcript = lines('line', 40)
    const history = transcript
    const screen = screenOf(transcript, rows)
    expect(screenSeam(keys(history), keys(screen))).toBe(40 - rows)
    expect(merge(history, screen)).toEqual(transcript)
  })

  it('keeps blank rows below a short screen without repeating its content', () => {
    const transcript = [...lines('line', 30), 'bash$ ']
    const screen = screenOf(transcript, rows, 3)
    const merged = merge(transcript, screen)
    expect(merged).not.toBeNull()
    expect(withoutTrailingBlanks(merged ?? [])).toEqual(transcript)
    expect(merged?.slice(-rows)).toEqual(screen)
  })

  it('adds output that arrived after the read exactly once', () => {
    for (let added = 1; added < rows; added += 1) {
      const transcript = lines('line', 60)
      const history = transcript.slice(0, 50)
      const screen = screenOf(transcript.slice(0, 50 + added), rows)
      expect(merge(history, screen)).toEqual(transcript.slice(0, 50 + added))
    }
  })

  it('replaces a redrawn inline footer instead of stacking an old one above new output', () => {
    const answer = lines('answer item', 20)
    const history = [...answer, '❯ ', '  status · working']
    const screen = [
      ...answer.slice(-4),
      'answer item 21',
      'answer item 22',
      '❯ ',
      '  status · idle',
    ]
    const merged = merge(history, screen)
    expect(merged).toEqual([...answer, 'answer item 21', 'answer item 22', '❯ ', '  status · idle'])
    expect(merged?.filter((row) => row.startsWith('❯'))).toHaveLength(1)
    expect(merged?.filter((row) => row.includes('status'))).toEqual(['  status · idle'])
  })

  it('refuses a seam when more than a screen of output arrived', () => {
    const transcript = lines('line', 80)
    const history = transcript.slice(0, 50)
    const screen = screenOf(transcript.slice(0, 50 + rows + 1), rows)
    expect(screenSeam(keys(history), keys(screen))).toBeNull()
  })

  it('refuses Herdr’s scrolled-back view as the latest screen', () => {
    const transcript = lines('line', 50)
    for (const offset of [1, 3, rows]) {
      const view = transcript.slice(50 - rows - offset, 50 - offset)
      expect(screenSeam(keys(transcript), keys(view))).toBeNull()
    }
  })

  it('refuses a seam that repeated content makes ambiguous', () => {
    const block = ['═══════', 'build ok', '═══════', 'build ok']
    const history = [...block, ...block, ...block, ...block]
    const screen = [...block, ...block]
    expect(screenSeam(keys(history), keys(screen))).toBeNull()
  })

  it('refuses a single agreeing row followed by a different screen', () => {
    const history = [...lines('line', 30), 'a', 'b', 'c', 'd']
    const screen = ['c', ...lines('redrawn', rows - 1)]
    expect(screenSeam(keys(history), keys(screen))).toBeNull()
  })

  it('refuses a seam made only of blank rows', () => {
    const history = [...lines('line', 30), 'p1', 'p2', '', '']
    const screen = ['', '', ...lines('new', rows - 2)]
    expect(screenSeam(keys(history), keys(screen))).toBeNull()
  })

  it('refuses a blank or unrelated screen', () => {
    const history = lines('line', 30)
    expect(screenSeam(keys(history), Array.from({ length: rows }, () => ''))).toBeNull()
    expect(screenSeam(keys(history), keys(lines('other', rows)))).toBeNull()
  })

  it('never duplicates or drops a line for any amount of new output', () => {
    for (let read = rows; read <= 30; read += 1) {
      for (let added = 0; added <= rows; added += 1) {
        const transcript = lines('row', read + added)
        const history = transcript.slice(0, read)
        const screen = screenOf(transcript, rows)
        const merged = merge(history, screen)
        if (added < rows) {
          expect(merged).toEqual(transcript)
        } else if (merged !== null) {
          expect(merged).toEqual(transcript)
        }
      }
    }
  })
})

describe('screenSeam with repeated rows', () => {
  const rows = 24
  const waiting = 'waiting for CI…'

  it('refuses a seam when the history ends with a line printed again and again', () => {
    // The screen's top could continue the run of "waiting" lines from any
    // of its rows; the first would drop the two new "waiting" lines.
    const history = [...lines('log line', 57), ...Array.from({ length: 20 }, () => waiting)]
    const transcript = [...history, waiting, waiting, ...lines('log line', 4, 58)]
    expect(screenSeam(keys(history), keys(screenOf(transcript, rows)))).toBeNull()
    // A screen that starts above the run is not ambiguous.
    const settled = [...history, ...lines('log line', 3, 58)]
    const screen = screenOf(settled, rows)
    const seam = screenSeam(keys(history), keys(screen))
    expect(seam === null ? null : mergeScreen(history, seam, screen)).toEqual(settled)
  })

  it('never drops or repeats a line of a log with runs of one repeated line', () => {
    const next = random(777)
    let trusted = 0
    for (let trial = 0; trial < 4_000; trial += 1) {
      const transcript: string[] = []
      let unique = 0
      while (transcript.length < 120) {
        if (next() < 0.15) {
          const run = 1 + Math.floor(next() * 40)
          for (let index = 0; index < run; index += 1) transcript.push(waiting)
        } else {
          transcript.push(`log line ${unique++}`)
        }
      }
      const read = 60 + Math.floor(next() * 40)
      const truth = transcript.slice(0, read + Math.floor(next() * rows))
      const history = transcript.slice(0, read)
      const screen = truth.slice(-rows)
      const seam = screenSeam(keys(history), keys(screen))
      if (seam === null) continue
      trusted += 1
      expect(mergeScreen(history, seam, screen)).toEqual(truth)
    }
    expect(trusted).toBeGreaterThan(1_000)
  })
})

describe('locateRows', () => {
  it('finds Herdr’s scrolled-back view in the history', () => {
    const transcript = lines('line', 100)
    const view = transcript.slice(60, 90)
    expect(locateRows(keys(transcript), keys(view), 70)).toBe(60)
  })

  it('prefers the match nearest the expected row for repeated rows', () => {
    const block = ['one', 'two', 'three']
    const history = [...block, 'x', ...block, 'y', ...block]
    expect(locateRows(keys(history), keys(block), 7)).toBe(8)
    expect(locateRows(keys(history), keys(block), 0)).toBe(0)
  })

  it('returns null when the view is not in the history', () => {
    expect(locateRows(keys(lines('a', 20)), keys(lines('b', 5)), 10)).toBeNull()
    expect(locateRows(keys(lines('a', 20)), ['', ''], 10)).toBeNull()
  })
})

describe('olderHistorySeam', () => {
  const cols = 20

  it('joins older plain lines onto the coloured rows at a whole line', () => {
    // Logical lines; one is wider than the pane and wraps into two rows.
    const transcript = [
      ...lines('old', 30),
      'a long line that wraps past twenty columns',
      ...lines('new', 12),
    ]
    const wrapped = (line: string) =>
      line.length <= cols
        ? [line]
        : [line.slice(0, cols), line.slice(cols, 2 * cols), line.slice(2 * cols)].filter(Boolean)
    const rendered = transcript.flatMap(wrapped)
    // The coloured read starts mid-way through the wrapped line.
    const firstRow = rendered.indexOf(transcript[30].slice(cols, 2 * cols))
    const rowTexts = rendered.slice(firstRow)
    const seam = olderHistorySeam(transcript, rowTexts, cols)
    expect(seam).not.toBeNull()
    const olderPart = transcript.slice(0, seam?.olderEnd)
    const rowPart = rowTexts.slice(seam?.rowsStart)
    // Everything once, in order: older lines, then rendered rows.
    expect([...olderPart.flatMap(wrapped), ...rowPart]).toEqual(rendered)
  })

  it('never anchors on the continuation of a wrapped row', () => {
    // A build log repeats a block; the second time a wide line wraps and
    // its continuation reads like the first block's own "done" line.
    const block = ['new 1', 'new 2', 'new 3', 'new 4']
    const wide = `${'x'.repeat(cols)}done`
    const tail = lines('tail', 3)
    const transcript = [...lines('old', 10), 'done', ...block, wide, ...block, ...tail]
    const rowTexts = ['x'.repeat(cols), 'done', ...block, ...tail]
    const seam = olderHistorySeam(transcript, rowTexts, cols)
    expect(seam).toEqual({
      olderEnd: transcript.length - block.length - tail.length,
      rowsStart: 2,
    })
  })

  const wrap = (line: string) => {
    if (line.length <= cols) return [line]
    const rows: string[] = []
    for (let index = 0; index < line.length; index += cols) {
      rows.push(line.slice(index, index + cols))
    }
    return rows
  }
  const join = (
    transcript: readonly string[],
    rowTexts: readonly string[],
    seam: { olderEnd: number; rowsStart: number } | null,
  ) =>
    seam && [
      ...transcript.slice(0, seam.olderEnd).flatMap(wrap),
      ...rowTexts.slice(seam.rowsStart),
    ]

  it('never joins at the wrong copy of a repeated block above wrapped lines', () => {
    const block = ['PASS unit', 'PASS lint', 'PASS e2e']
    const detail = (index: number) => `detail ${index} `.padEnd(3 * cols, '.')
    const transcript = [
      ...lines('old', 10),
      'run 1',
      ...block,
      'run 2',
      ...block,
      ...[0, 1, 2, 3].map(detail),
      'bash$ ',
    ]
    const rendered = transcript.flatMap(wrap)
    // Herdr coloured only the rows from "run 2" on; the only anchor is the
    // repeated block, so there is no join rather than a wrong one.
    const rowTexts = rendered.slice(rendered.indexOf('run 2'))
    expect(olderHistorySeam(transcript, rowTexts, cols)).toBeNull()
    // A line that appears once anchors it.
    const summary = [...transcript]
    summary.splice(summary.indexOf('run 2') + 4, 0, '3 passed in run 2')
    const summaryRows = summary.flatMap(wrap)
    const summaryTexts = summaryRows.slice(summaryRows.indexOf('run 2'))
    expect(join(summary, summaryTexts, olderHistorySeam(summary, summaryTexts, cols))).toEqual(
      summaryRows,
    )
  })

  it('joins every log with repeated blocks and wrapped lines exactly once, or not at all', () => {
    const next = random(4242)
    let found = 0
    for (let trial = 0; trial < 5_000; trial += 1) {
      const transcript: string[] = []
      let unique = 0
      while (transcript.length < 200) {
        const choice = next()
        if (choice < 0.1) {
          const times = 1 + Math.floor(next() * 6)
          for (let time = 0; time < times; time += 1) transcript.push('', 'step ok', 'waiting')
        } else if (choice < 0.2) {
          transcript.push(`long ${unique++} `.padEnd(cols + 1 + Math.floor(next() * 30), 'y'))
        } else {
          transcript.push(`line ${unique++}`)
        }
      }
      const rendered = transcript.flatMap(wrap)
      const rowTexts = rendered.slice(-(30 + Math.floor(next() * 100)))
      const seam = olderHistorySeam(transcript, rowTexts, cols)
      if (!seam) continue
      found += 1
      expect(join(transcript, rowTexts, seam)).toEqual(rendered)
    }
    expect(found).toBeGreaterThan(4_000)
  })

  it('gives up rather than guess when no row can anchor the join', () => {
    const full = Array.from({ length: 20 }, (_, index) => `${index}`.padEnd(cols, 'x'))
    expect(olderHistorySeam(full, full, cols)).toBeNull()
    expect(olderHistorySeam(lines('a', 10), lines('b', 10), cols)).toBeNull()
  })
})

describe('olderLinesStart', () => {
  it('keeps the newest older lines whose wrapped rows fit', () => {
    const wide = 'x'.repeat(25)
    // 10 columns: each wide line takes 3 rows, a short one 1.
    expect(olderLinesStart([wide, wide, 'a', wide], 10, 7)).toBe(1)
    expect(olderLinesStart([wide, 'a', ''], 10, 5)).toBe(0)
    expect(olderLinesStart([wide, wide], 10, 2)).toBe(2)
  })
})

describe('serializeRow', () => {
  interface FakeCell {
    chars: string
    width?: number
    fg?: number
    bg?: number
    fgMode?: 'default' | 'palette' | 'rgb'
    bgMode?: 'default' | 'palette' | 'rgb'
    bold?: boolean
  }
  const cellOf = (fake: FakeCell): SerializableCell => ({
    getChars: () => fake.chars,
    getWidth: () => fake.width ?? 1,
    getFgColor: () => fake.fg ?? 0,
    getBgColor: () => fake.bg ?? 0,
    isFgRGB: () => fake.fgMode === 'rgb',
    isBgRGB: () => fake.bgMode === 'rgb',
    isFgPalette: () => fake.fgMode === 'palette',
    isBgPalette: () => fake.bgMode === 'palette',
    isBold: () => (fake.bold ? 1 : 0),
    isDim: () => 0,
    isItalic: () => 0,
    isUnderline: () => 0,
    isBlink: () => 0,
    isInverse: () => 0,
    isInvisible: () => 0,
    isStrikethrough: () => 0,
    isOverline: () => 0,
  })
  const lineOf = (cells: FakeCell[]) => ({
    getCell: (x: number) => (x < cells.length ? cellOf(cells[x]) : undefined),
  })

  it('keeps colours and attributes and drops trailing blanks', () => {
    const line = lineOf([
      { chars: 'o', fgMode: 'palette', fg: 2, bold: true },
      { chars: 'k', fgMode: 'palette', fg: 2, bold: true },
      { chars: ' ' },
      { chars: '⏺', width: 2, fgMode: 'rgb', fg: 0x10_20_30 },
      { chars: '', width: 0 },
      { chars: 'x' },
      { chars: ' ' },
      { chars: '' },
    ])
    const row = serializeRow(line, 8)
    expect(row).toBe(
      '\u001b[0;1;38;5;2mok\u001b[0m \u001b[0;38;2;16;32;48m⏺\u001b[0mx\u001b[0m',
    )
    expect(stripTerminalEscapes(row)).toBe('ok ⏺x')
  })

  it('keeps a styled blank run such as a status bar background', () => {
    const line = lineOf([
      { chars: 'a' },
      { chars: ' ', bgMode: 'palette', bg: 4 },
      { chars: ' ' },
    ])
    expect(serializeRow(line, 3)).toBe('a\u001b[0;48;5;4m \u001b[0m')
    expect(serializeRow(lineOf([{ chars: ' ' }, { chars: '' }]), 2)).toBe('')
  })
})

describe('refreshSeam', () => {
  const rows = 8
  // What history holds after a read: output, then an inline footer.
  const painted = (output: readonly string[], status: string) => [
    ...output,
    `⠋ Working (${status})`,
    '  ctx 42%',
  ]
  const refresh = (history: readonly string[], read: readonly string[]) => {
    const join = refreshSeam(keys(history), keys(read), rows)
    return join === null
      ? null
      : mergeScreen(history, join.seam, read.slice(join.from))
  }

  it('joins a read taken after more than a screen of output exactly once', () => {
    const output = lines('inl', 400)
    const history = painted(output.slice(0, 200), '11s')
    // The live screen alone cannot join: it is all new rows.
    const screen = painted(output.slice(0, 380), '19s').slice(-rows)
    expect(screenSeam(keys(history), keys(screen))).toBeNull()
    // Herdr's read holds only its latest rows, from inside the history.
    for (const readFrom of [0, 1, 57, 150, 199 - rows]) {
      const read = painted(output.slice(readFrom, 380), '19s')
      expect(refresh(history, read)).toEqual(painted(output.slice(0, 380), '19s'))
    }
  })

  it('replaces a stale footer and keeps rows the read no longer holds', () => {
    const output = lines('inl', 120)
    const history = [...lines('older plain', 30), ...painted(output.slice(0, 60), '5s')]
    const read = painted(output.slice(40), '9s')
    const merged = refresh(history, read)
    expect(merged).toEqual([...lines('older plain', 30), ...painted(output, '9s')])
    expect(merged?.filter((row) => row.includes('Working'))).toEqual(['⠋ Working (9s)'])
  })

  it('adds nothing when no new output arrived', () => {
    const history = painted(lines('inl', 50), '5s')
    expect(refresh(history, history)).toEqual(history)
  })

  it('refuses a read that no longer reaches back into the history', () => {
    const output = lines('inl', 3_000)
    const history = painted(output.slice(0, 1_000), '5s')
    expect(refresh(history, painted(output.slice(1_990), '60s'))).toBeNull()
    // A read that starts just after the history shares no row with it, so
    // a missing row could not be told apart from none.
    expect(refresh(history, painted(output.slice(1_000), '60s'))).toBeNull()
  })

  it('refuses a read that could start at two places', () => {
    const block = ['same a', 'same b', 'same c', 'same d']
    const history = [...block, ...block, ...block, ...block, ...block, 'tail 1', 'tail 2']
    expect(refresh(history, [...block, ...block, 'new 1'])).toBeNull()
  })

  it('does not trust a start inside the last screen on one coincidental row', () => {
    const history = painted(lines('inl', 50), '5s')
    // The read's first row happens to equal the history's last row only.
    expect(refresh(history, ['  ctx 42%', ...lines('other', 40)])).toBeNull()
    expect(refreshSeam(keys(history), [], rows)).toBeNull()
  })
})

describe('prependSeam', () => {
  const rows = 8
  const prepend = (older: readonly string[], history: readonly string[]) => {
    const top = prependSeam(keys(older), keys(history), rows)
    return top === null ? null : [...older.slice(0, top), ...history]
  }

  it('puts the longer read above the entry rows exactly once', () => {
    const transcript = [...lines('row', 300), 'bash$ ']
    const history = transcript.slice(-2 * rows)
    expect(prepend(transcript, history)).toEqual(transcript)
    // The entry rows may already have been merged with a newer screen.
    const merged = [...transcript.slice(-2 * rows, -1), ...lines('new', 3), 'bash$ ']
    expect(prepend(transcript, merged)).toEqual([...transcript.slice(0, -1), ...lines('new', 3), 'bash$ '])
  })

  it('accepts a longer read taken after more output, whose last screen differs', () => {
    const output = lines('inl', 200)
    const history = [...output.slice(100, 130), '⠋ Working (3s)', '  ctx']
    const older = [...output.slice(0, 180), '⠋ Working (9s)', '  ctx']
    expect(prepend(older, history)).toEqual([...output.slice(0, 100), ...history])
  })

  it('finds nothing to add when the longer read starts at the same row', () => {
    const transcript = lines('row', 20)
    expect(prependSeam(keys(transcript), keys(transcript.slice(0)), rows)).toBe(0)
  })

  it('refuses a history it cannot place, or can place twice', () => {
    expect(prepend(lines('row', 100), lines('other', 20))).toBeNull()
    const block = ['same a', 'same b', 'same c', 'same d', 'same e']
    const older = [...block, ...block, ...block, ...block]
    expect(prepend(older, [...block, 'tail'])).toBeNull()
    // Final rows that disagree are not an older paint.
    const history = [...lines('row', 30, 50)]
    history[5] = 'row changed'
    expect(prepend(lines('row', 100), history)).toBeNull()
  })

  it('reads a couple of screens on entry, within Herdr\'s cap', () => {
    expect(historyEntryLines(43)).toBe(86)
    expect(historyEntryLines(0)).toBe(2)
    expect(historyEntryLines(900)).toBe(HISTORY_ROW_LINES)
  })
})

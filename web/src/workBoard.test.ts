import { describe, expect, it } from 'vitest'
import type { WorkerCandidate } from './types'
import {
  EMPTY_FILTER,
  actionDialog,
  boardCommand,
  filterFromHash,
  filterToHash,
  formatAgo,
  ageTooltip,
  agentName,
  cardSteps,
  ACTION_LABELS,
  boardAnnouncement,
  outOfDateLabel,
  isQuestion,
  groupItems,
  headerDetails,
  headerLine,
  heatFor,
  isOutOfDate,
  moveItem,
  newBaseline,
  reconcileBoard,
  workersFingerprint,
  countItems,
  helperItems,
  isOnMain,
  olderItems,
  reviewActions,
  reviewKey,
  reviewQueue,
  statusWithAge,
  windowLine,
  archiveBanner,
  boardList,
  rowLine,
  olderLine,
  type BoardItem,
  type WorkBoard,
} from './workBoard'

function item(overrides: Partial<BoardItem>): BoardItem {
  return {
    id: 'a:1',
    kind: 'assignment',
    column: 'doing',
    section: null,
    rule: 'R2',
    chips: [],
    project_id: 'alpha',
    project_name: 'alpha',
    worker_id: 'w1',
    assignment_id: '1',
    title: 'worker-ui',
    objective: 'Edit the form',
    status_line: 'Editing form',
    summary: 'Working on it right now',
    next_step: 'Nothing for now; let it work.',
    labels: ['working'],
    details: [],
    next_actions: [],
    age_ms: 60_000,
    age_source: 'worker updated',
    heat: 'hot',
    ai: null,
    actions: ['open_terminal', 'open_project'],
    rollup: false,
    pending: false,
    dead_count: null,
    ...overrides,
  }
}

function board(items: BoardItem[], overrides: Partial<WorkBoard> = {}): WorkBoard {
  return {
    revision: 3,
    generation_id: 'g',
    state: 'idle',
    stage: null,
    generated_at_unix_ms: 1_000_000,
    fingerprint: '5-100',
    mode: 'deterministic',
    model_available: false,
    thresholds: { fresh_ms: 900_000, review_window_ms: 86_400_000, cold_ms: 604_800_000 },
    warnings: [],
    error: null,
    items,
    todos: [],
    projects: [
      { id: 'alpha', name: 'alpha', status_line: '', orchestrator_column: 'stale', counts: { doing: 0, needs_you: 0, stale_needs_work: 0, stale_likely_done: 0, close: 0, closed: 0 } },
      { id: 'beta', name: 'beta', status_line: '', orchestrator_column: 'stale', counts: { doing: 0, needs_you: 0, stale_needs_work: 0, stale_likely_done: 0, close: 0, closed: 0 } },
    ],
    ...overrides,
  }
}

function candidate(id: string, sequence: string, updated: number, availability = 'assigned'): WorkerCandidate {
  return {
    availability,
    worker: {
      id,
      runtime: { state_change_sequence: sequence },
      updated_at_unix_ms: updated,
    },
  } as unknown as WorkerCandidate
}

describe('groupItems', () => {
  const items = [
    item({ id: 'doing' }),
    item({ id: 'needs', column: 'needs_you' }),
    item({ id: 'work', column: 'stale', section: 'needs_work' }),
    item({ id: 'ambiguous', column: 'stale', section: 'ambiguous' }),
    item({ id: 'done?', column: 'stale', section: 'likely_done' }),
    item({ id: 'close', column: 'close' }),
    item({ id: 'closed', column: 'closed' }),
    item({ id: 'orch', kind: 'orchestrator', rollup: true, column: 'stale' }),
  ]

  it('places cards in four columns with stale sub-sections and hides roll-ups', () => {
    const columns = groupItems(items, EMPTY_FILTER)
    expect(columns.map((column) => column.id)).toEqual(['doing', 'needs_you', 'stale', 'close'])
    const stale = columns[2]
    expect(stale?.sections?.[0]?.items.map(({ id }) => id)).toEqual(['work', 'ambiguous'])
    expect(stale?.sections?.[1]?.items.map(({ id }) => id)).toEqual(['done?'])
    expect(columns.flatMap((column) => column.items).some(({ id }) => id === 'orch')).toBe(false)
  })

  it('shows Closed only on request', () => {
    const columns = groupItems(items, { ...EMPTY_FILTER, showClosed: true })
    expect(columns.at(-1)?.items.map(({ id }) => id)).toEqual(['closed'])
  })

  it('filters by project, loose agents and text', () => {
    const mixed = [
      item({ id: 'a', project_id: 'alpha' }),
      item({ id: 'b', project_id: 'beta', title: 'api-fix' }),
      item({ id: 'l', kind: 'loose_agent', project_id: null, project_name: null }),
    ]
    const ids = (filter: typeof EMPTY_FILTER) =>
      groupItems(mixed, filter).flatMap((column) => column.items.map(({ id }) => id))
    expect(ids({ ...EMPTY_FILTER, projects: ['beta'] })).toEqual(['b'])
    expect(ids({ ...EMPTY_FILTER, loose: true })).toEqual(['l'])
    expect(ids({ ...EMPTY_FILTER, projects: ['alpha'], loose: true })).toEqual(['a', 'l'])
    expect(ids({ ...EMPTY_FILTER, text: 'API' })).toEqual(['b'])
  })

})

describe('filter hash', () => {
  it('round-trips', () => {
    const filter = { projects: ['alpha', 'beta'], loose: true, text: 'retry test', showClosed: true }
    expect(filterFromHash(filterToHash(filter))).toEqual(filter)
    expect(filterToHash(EMPTY_FILTER)).toBe('#board')
    expect(filterFromHash('#board')).toEqual(EMPTY_FILTER)
    expect(filterFromHash('#other')).toBeNull()
  })
})

describe('heat and age', () => {
  it('buckets ages', () => {
    expect(heatFor(59 * 60_000)).toBe('hot')
    expect(heatFor(60 * 60_000)).toBe('warm')
    expect(heatFor(86_400_000)).toBe('cool')
    expect(heatFor(7 * 86_400_000)).toBe('cold')
    expect(formatAgo(30_000)).toBe('just now')
    expect(formatAgo(5 * 60_000)).toBe('5 min ago')
    expect(formatAgo(5 * 3_600_000)).toBe('5 hr ago')
    expect(formatAgo(86_400_000)).toBe('1 day ago')
    expect(formatAgo(9 * 86_400_000)).toBe('9 days ago')
    expect(ageTooltip({ age_ms: 3 * 86_400_000, age_source: 'receipt', heat: 'cool' })).toBe(
      'Last activity 3 days ago (it left a report on its finished work), quiet for days',
    )
  })
})

describe('keyboard map', () => {
  it('maps keys and ignores modified keys', () => {
    expect(boardCommand({ key: 'j' })).toEqual({ kind: 'move', delta: 1 })
    expect(boardCommand({ key: 'k' })).toEqual({ kind: 'move', delta: -1 })
    expect(boardCommand({ key: ' ' })).toEqual({ kind: 'toggle' })
    expect(boardCommand({ key: 'l' })).toBeNull()
    expect(boardCommand({ key: '3' })).toBeNull()
    expect(boardCommand({ key: 't' })).toBeNull()
    expect(boardCommand({ key: 'Enter' })).toEqual({ kind: 'open' })
    expect(boardCommand({ key: 'd' })).toEqual({ kind: 'done' })
    expect(boardCommand({ key: 'x' })).toBeNull()
    expect(boardCommand({ key: 'g' })).toEqual({ kind: 'generate' })
    expect(boardCommand({ key: '/' })).toEqual({ kind: 'filter' })
    expect(boardCommand({ key: 'Escape' })).toEqual({ kind: 'back' })
    expect(boardCommand({ key: 'j', metaKey: true })).toBeNull()
    expect(boardCommand({ key: 'q' })).toBeNull()
  })
})

describe('out-of-date fingerprint', () => {
  it('matches the server format and flags changes', () => {
    const workers = [candidate('a', '5', 100), candidate('b', '3', 90)]
    expect(workersFingerprint(workers)).toBe('5-100')
    const current = board([])
    expect(isOutOfDate(current, workers, false)).toBe(false)
    expect(isOutOfDate(current, [candidate('a', '6', 100)], false)).toBe(true)
    expect(isOutOfDate(current, workers, true)).toBe(true)
    expect(isOutOfDate(board([], { state: 'empty', fingerprint: null }), workers, true)).toBe(false)
  })
})

describe('actions', () => {
  it('route every write through an existing dialog', () => {
    expect(actionDialog('open_terminal')).toBe('navigate')
    expect(actionDialog('mark_done')).toBe('completion')
    expect(actionDialog('end_session')).toBe('end-session')
    expect(actionDialog('archive_project')).toBe('archive-project')
    expect(actionDialog('clean_up')).toBe('hide-stale')
    expect(actionDialog('resume')).toBe('resume')
  })

  it('moves a card optimistically and reconciles lifecycle changes', () => {
    const current = board([item({ id: 'a:1', column: 'stale', section: 'likely_done' })])
    expect(moveItem(current, 'a:1', 'close').items?.[0]?.column).toBe('close')
    const completed = reconcileBoard(current, [{ id: '1', lifecycle: 'completed' }], [])
    expect(completed.items?.[0]?.column).toBe('close')
    const ended = reconcileBoard(current, [], [candidate('w1', '1', 1, 'ended')])
    expect(ended.items?.[0]?.column).toBe('closed')
    const asked = reconcileBoard(current, [], [], () => true)
    expect(asked.items?.[0]?.column).toBe('needs_you')
    expect(asked.items?.[0]?.chips).toContain('question')
    expect(reconcileBoard(current, [{ id: '1', lifecycle: 'active' }], [])).toBe(current)
  })

  it('reconciles only changes since the board arrived and keeps counts and to-dos in step', () => {
    const current = board([item({ id: 'a:1', column: 'stale', section: 'needs_work' })])
    const baseline = newBaseline(current.revision)
    // Known at generation time (already completed, question already asked):
    // the server's placement stands.
    expect(
      reconcileBoard(current, [{ id: '1', lifecycle: 'completed' }], [], () => true, baseline),
    ).toBe(current)
    // A later change moves the card, recounts and adds no duplicate.
    const later = reconcileBoard(
      current,
      [{ id: '1', lifecycle: 'cancelled' }],
      [],
      () => true,
      baseline,
    )
    expect(later.items?.[0]?.column).toBe('closed')
    expect(later.counts?.closed).toBe(1)
    expect(later.counts?.stale_needs_work).toBe(0)
    // A new question adds a now Reply to-do.
    const fresh = newBaseline(current.revision)
    reconcileBoard(current, [], [], () => false, fresh)
    const asked = reconcileBoard(current, [], [], () => true, fresh)
    expect(asked.items?.[0]?.column).toBe('needs_you')
    expect(asked.counts?.needs_you).toBe(1)
    expect(asked.todos?.[0]).toMatchObject({
      urgency: 'now',
      kind: 'reply',
      text: 'Answer the question from \u201cworker-ui\u201d',
    })
    expect(asked.items?.[0]).toMatchObject({
      summary: 'Has a question for you',
      next_step: 'Open it and answer the question.',
    })
    // The summary says it; no chip repeats it.
    expect(asked.items?.[0]?.labels).not.toContain('has a question')
    expect(agentName({ title: 'api-fix' })).toBe('\u201capi-fix\u201d')
  })
})

describe('header line', () => {
  it('says when and how the board was built', () => {
    expect(headerLine(board([], { state: 'empty', generated_at_unix_ms: null }), 0)).toBe(
      'Not made yet. Press Refresh board to read your open agents.',
    )
    expect(headerDetails(board([], { state: 'empty', generated_at_unix_ms: null }))).toContain(
      'not configured',
    )
    const line = headerLine(
      board([], {
        mode: 'model',
        model_available: true,
        model: { enabled: true, calls: 3, input_tokens: 1000, output_tokens: 200, usd: 0.0123, error: null, skipped: false, batches_done: 3, batches_total: 3 },
      }),
      1_000_000 + 120_000,
    )
    expect(line).toMatch(/^Updated \d{1,2}:\d{2}/)
    expect(line).not.toMatch(/^Updated 0\d/)
    // The model ran but no card has an AI note: the header does not claim AI summaries.
    expect(line).toContain('(2 min ago) \u00b7 AI summaries ran but added nothing')
    const withLines = headerLine(
      board(
        [
          item({
            ai: { verdict: 'finished', confidence: 0.9, evidence: 'tail', quote: '', next_actions: [], title: '', about: 'Upload retry fix', status: 'Done', your_move: '' },
          } as Partial<BoardItem>),
        ],
        {
          mode: 'model',
          model_available: true,
          model: { enabled: true, calls: 3, input_tokens: 1000, output_tokens: 200, usd: 0.0123, error: null, skipped: false, batches_done: 3, batches_total: 3 },
        },
      ),
      1_000_000 + 120_000,
    )
    expect(withLines).toContain('(2 min ago) \u00b7 With AI summaries')
    expect(line).not.toMatch(/ \. |built-in/)
    // Calls, tokens and cost stay off the face, in Details.
    expect(line).not.toMatch(/call|token|\$/)
    const details = headerDetails(
      board([], {
        mode: 'model',
        model_available: true,
        model: { enabled: true, calls: 3, input_tokens: 1000, output_tokens: 200, usd: 0.0123, error: null, skipped: false, batches_done: 3, batches_total: 3 },
      }),
    )
    expect(details).toContain('3 calls')
    expect(details).toContain('1,200 tokens')
    expect(details).toContain('$0.012')
  })
})

describe('plain card copy helpers', () => {
  it('tells questions from steps and skips the step "What to do" quotes', () => {
    expect(isQuestion('Use the v2 invoice schema? ')).toBe(true)
    expect(isQuestion('Add a retry test')).toBe(false)
    expect(
      cardSteps({
        next_actions: ['Add a retry test for large uploads.', 'Document the limit'],
        next_step: 'Tell it to continue with \u201cAdd a retry test for large uploads\u201d, or close it if you are done.',
      }),
    ).toEqual(['Document the limit'])
  })
})

describe('readability round 2', () => {
  it('announces with plural-aware words', () => {
    const counts = { doing: 1, needs_you: 1, stale_needs_work: 2, stale_likely_done: 1, close: 1, closed: 0 }
    const board = { state: 'idle', counts } as unknown as WorkBoard
    expect(boardAnnouncement(board)).toBe(
      'Work board ready: 1 needs you, 1 working now, 3 gone quiet, 1 finished and ready to close',
    )
    expect(boardAnnouncement({ ...board, counts: { ...counts, needs_you: 2 } })).toContain('2 need you')
  })

  it('says why the board is out of date with the one refresh verb', () => {
    expect(outOfDateLabel(null)).toBe('New activity · Refresh board')
    expect(outOfDateLabel({ generated_at_unix_ms: Date.UTC(2026, 9, 26, 15, 16) })).toMatch(
      /^New activity since .+ · Refresh board$/,
    )
  })

  it('uses one ellipsis character and agent nouns on buttons', () => {
    for (const label of Object.values(ACTION_LABELS)) {
      expect(label).not.toContain('...')
      expect(label.toLowerCase()).not.toContain('session')
    }
    expect(ACTION_LABELS.open_terminal).toBe('Open terminal')
    expect(ACTION_LABELS.end_session).toBe('Close agent…')
  })

  it('does not list the step the next step already quotes, whatever its case', () => {
    expect(
      cardSteps({ next_actions: ['Use the v2 schema?', 'Rerun it'], next_step: 'Answer its question: use the v2 schema?' }),
    ).toEqual(['Rerun it'])
  })
})

describe('board v3: relevance, review, helpers, card lines', () => {
  const recent = item({ id: 'a:recent', relevance: 'recent', activity_known: true })
  const older = item({
    id: 'w:old',
    kind: 'worker',
    column: 'stale',
    section: 'needs_work',
    relevance: 'older',
    age_ms: 9 * 86_400_000,
    activity_known: true,
    actions: ['open_terminal', 'end_session', 'open_project'],
  })
  const reviewed = item({ id: 'w:seen', column: 'stale', relevance: 'older', reviewed: true, age_ms: 20 * 86_400_000 })
  const helper = item({ id: 'l:tmp', kind: 'loose_agent', column: 'needs_you', helper: 'A temporary session Yard started' })

  it('keeps Older work and helpers off the columns and the counts', () => {
    const all = [recent, older, reviewed, helper]
    expect(isOnMain(recent)).toBe(true)
    expect(isOnMain(older)).toBe(false)
    expect(isOnMain(helper)).toBe(false)
    // An older server without the field: everything stays on the board.
    expect(isOnMain(item({ id: 'x' }))).toBe(true)
    const shown = groupItems(all, EMPTY_FILTER).flatMap((column) => column.items.map((card) => card.id))
    expect(shown).toEqual(['a:recent'])
    expect(countItems(all).needs_you).toBe(0)
    expect(olderItems(all, EMPTY_FILTER).map((card) => card.id)).toEqual(['w:old', 'w:seen'])
    expect(reviewQueue(all).map((card) => card.id)).toEqual(['w:old'])
    expect(helperItems(all, EMPTY_FILTER).map((card) => card.id)).toEqual(['l:tmp'])
  })

  it('writes the Status line with an age only when the age is known', () => {
    expect(statusWithAge({ status: 'Gone quiet', summary: '', activity_known: true, age_ms: 2 * 3_600_000, column: 'stale' })).toBe(
      'Gone quiet \u00b7 2 hr ago',
    )
    expect(statusWithAge({ status: 'Gone quiet', summary: '', activity_known: false, age_ms: 0, column: 'stale' })).toBe('Gone quiet')
    expect(statusWithAge({ status: 'Working now', summary: '', activity_known: true, age_ms: 0, column: 'doing' })).toBe('Working now')
    // Nothing known beyond the age: only the age (never the summary or a placeholder).
    expect(statusWithAge({ status: '', summary: 'Quiet, with no task assigned', activity_known: true, age_ms: 4 * 86_400_000, column: 'stale' })).toBe(
      '4 days ago',
    )
  })

  it('maps Done and Archive to existing dialogs only', () => {
    expect(reviewActions({ actions: ['mark_done', 'end_session'] })).toEqual({ done: 'mark_done', archive: 'end_session' })
    expect(reviewActions({ actions: ['end_session'] })).toEqual({ done: 'end_session', archive: null })
    expect(reviewActions({ actions: ['open_terminal', 'hide'] })).toEqual({ done: null, archive: 'hide' })
    expect(reviewActions({ actions: ['open_project'] })).toEqual({ done: null, archive: null })
  })

  it('has review shortcuts and shows the window', () => {
    expect(['k', 'd', 'a', 's', 'Escape', 'x'].map(reviewKey)).toEqual(['keep', 'done', 'archive', 'skip', 'close', null])
    expect(
      windowLine({ fresh_ms: 1, review_window_ms: 1, cold_ms: 1, recent_ms: 3 * 86_400_000, waiting_ms: 7 * 86_400_000, suggest_archive_ms: 14 * 86_400_000 }),
    ).toContain('last 3 days')
  })
})

describe('board v3 round 1', () => {
  const older = (id: string, days: number, extra: Partial<BoardItem> = {}) =>
    item({ id, column: 'stale', section: 'likely_done', relevance: 'older', age_ms: days * 86_400_000, suggest_archive: days >= 14, ...extra } as Partial<BoardItem>)

  it('orders Older work: suggest archive oldest first, then quiet newest first', () => {
    const items = [older('q4', 4), older('a20', 20), older('q6', 6), older('a39', 39), older('a25', 25)]
    expect(olderItems(items, EMPTY_FILTER).map((entry) => entry.id)).toEqual(['a39', 'a25', 'a20', 'q4', 'q6'])
    expect(reviewQueue(items).map((entry) => entry.id)).toEqual(['a39', 'a25', 'a20', 'q4', 'q6'])
  })

  it('says what Older work holds in one line, and the review banner names the age', () => {
    const items = [older('a39', 39), older('a20', 20), older('q4', 4), older('done', 4, { column: 'close', section: null })]
    expect(olderLine(items, undefined)).toBe('2 untouched 2+ weeks, 2 quiet.')
    expect(olderLine([older('q4', 4)], undefined)).toBe('1 quiet.')
    expect(olderLine([], undefined)).toBe('')
    expect(archiveBanner(older('a20', 20))).toBe('Untouched 20 days \u2014 suggest archive.')
    expect(archiveBanner(older('q4', 4))).toBe('')
  })

  it('puts chats with no task in their own quiet group and drops empty groups', () => {
    const items = [
      item({ id: 'stalled', column: 'stale', section: 'needs_work', relevance: 'recent' } as Partial<BoardItem>),
      item({ id: 'chat', column: 'stale', section: 'needs_work', chips: ['loose agent'], relevance: 'recent' } as Partial<BoardItem>),
    ]
    const stale = groupItems(items, EMPTY_FILTER).find((column) => column.id === 'stale')
    expect(stale?.sections?.map((section) => [section.label, section.items.map((entry) => entry.id)])).toEqual([
      ['Stalled, not finished', ['stalled']],
      ['Open, no task', ['chat']],
    ])
  })

  it('splits Needs you into answer first, then review', () => {
    const items = [
      item({ id: 'r', column: 'needs_you', need: 'review', relevance: 'recent' } as Partial<BoardItem>),
      item({ id: 'q', column: 'needs_you', need: 'answer', relevance: 'recent' } as Partial<BoardItem>),
    ]
    const needs = groupItems(items, EMPTY_FILTER).find((column) => column.id === 'needs_you')
    expect(needs?.sections?.map((section) => [section.label, section.items.map((entry) => entry.id)])).toEqual([
      ['Blocked or has a question', ['q']],
      ['Finished, review and close', ['r']],
    ])
    expect(needs?.items.map((entry) => entry.id)).toEqual(['q', 'r'])
  })

  it('calls hiding a chat "Archive", as the review does', () => {
    expect(ACTION_LABELS.hide).toBe('Archive\u2026')
  })
})

describe('single list', () => {
  const cards = [
    item({ id: 'w', column: 'doing', status: 'Working: editing the form', your_move: 'Nothing — it’s working' } as Partial<BoardItem>),
    item({ id: 'r', column: 'needs_you', need: 'review', relevance: 'recent', your_move: 'Review what it did, then mark it done.' } as Partial<BoardItem>),
    item({ id: 'q', column: 'needs_you', need: 'answer', relevance: 'recent', your_move: 'Answer its question: use v2?' } as Partial<BoardItem>),
    item({ id: 'c', column: 'close', relevance: 'recent', your_move: 'Close it to free the terminal.' } as Partial<BoardItem>),
    item({ id: 'd', column: 'stale', section: 'likely_done', relevance: 'recent', your_move: 'Mark it done or close it.' } as Partial<BoardItem>),
    item({ id: 's', column: 'stale', section: 'needs_work', relevance: 'recent', status: 'Idle, waiting for work', your_move: 'Nothing — it’s idle' } as Partial<BoardItem>),
    item({ id: 'o', column: 'needs_you', need: 'answer', relevance: 'older' } as Partial<BoardItem>),
    item({ id: 'h', column: 'needs_you', need: 'answer', relevance: 'recent', helper: 'A Yard summary worker' } as Partial<BoardItem>),
    item({ id: 'x', column: 'closed', relevance: 'recent', status: 'Closed' } as Partial<BoardItem>),
    item({ id: 'roll', kind: 'orchestrator', rollup: true, column: 'needs_you', relevance: 'recent' } as Partial<BoardItem>),
  ]

  it('puts answers, then reviews, then finished work under Needs you; older work, helpers and roll-ups stay off', () => {
    const list = boardList(cards, EMPTY_FILTER)
    expect(list.needs.map(({ id }) => id)).toEqual(['q', 'r', 'c'])
    expect(list.working.map(({ id }) => id)).toEqual(['w'])
    expect(list.quiet.map(({ id }) => id)).toEqual(['s', 'd'])
    expect(list.closed).toEqual([])
    expect(boardList(cards, { ...EMPTY_FILTER, showClosed: true }).closed.map(({ id }) => id)).toEqual(['x'])
  })

  it('says one sentence per row: the move, what it is doing, or the status when there is nothing to do', () => {
    const by = (id: string) => cards.find((card) => card.id === id) as BoardItem
    expect(rowLine(by('q'), 'needs')).toBe('Answer its question: use v2?')
    expect(rowLine(by('w'), 'working')).toBe('Editing the form')
    expect(rowLine(by('s'), 'quiet')).toBe('Idle, waiting for work')
    expect(rowLine(by('d'), 'quiet')).toBe('Mark it done or close it.')
    expect(rowLine(item({ your_move: 'Refresh your AWS credentials. Then tell it to continue.' }), 'needs')).toBe(
      'Refresh your AWS credentials.',
    )
  })
})

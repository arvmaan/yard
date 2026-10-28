// Work board: types of `GET /api/v1/work-board` and the pure helpers the
// tab uses (grouping, filters, heat, keyboard map, to-dos, staleness).

import type { WorkerCandidate } from './types'

export type BoardColumn = 'doing' | 'needs_you' | 'stale' | 'close' | 'closed'
export type BoardSection = 'needs_work' | 'ambiguous' | 'likely_done'
export type BoardHeat = 'hot' | 'warm' | 'cool' | 'cold'
export type BoardItemKind =
  | 'assignment'
  | 'worker'
  | 'loose_agent'
  | 'dead_aggregate'
  | 'orchestrator'
export type BoardAction =
  | 'open_terminal'
  | 'open_project'
  | 'mark_done'
  | 'resume'
  | 'end_session'
  | 'archive_project'
  | 'clean_up'
  | 'hide'
export type TodoUrgency = 'now' | 'soon' | 'later'

export interface BoardAiNote {
  verdict: 'needs_work' | 'finished' | 'waiting_on_user' | 'unclear'
  confidence: number
  evidence: string
  quote: string
  next_actions: string[]
  /** The AI's title and three card lines (empty when it gave none). */
  title?: string
  about?: string
  status?: string
  your_move?: string
}

/** Why a card is on the main board (or in Older work). */
export type BoardRelevance = 'recent' | 'waiting' | 'pinned' | 'older'
export type BoardNeed = 'answer' | 'review'

export interface BoardItem {
  id: string
  kind: BoardItemKind
  column: BoardColumn
  section: BoardSection | null
  rule: string
  chips: string[]
  project_id: string | null
  project_name: string | null
  worker_id: string | null
  assignment_id: string | null
  title: string
  /** Where the title came from ("conversation title", "task", "untitled", ...). */
  title_source?: string
  objective: string
  /** The three card lines: what the work is, where it is at, what to do. */
  about?: string
  status?: string
  your_move?: string
  /** Whether `age_ms` is a real age (otherwise ages are not shown). */
  activity_known?: boolean
  /** Main board (recent, waiting, pinned) or the collapsed Older work. */
  relevance?: BoardRelevance
  /** Why this is a helper (hidden unless "Show helpers"), if it is one. */
  helper?: string | null
  /** Kept on the board by the user. */
  pinned?: boolean
  /** Skipped in a review and unchanged since. */
  reviewed?: boolean
  /** Untouched past the suggest-archive threshold. */
  suggest_archive?: boolean
  /** Needs you: a question or blocker to answer, or finished work to review. */
  need?: BoardNeed | null
  provider?: string | null
  /** The agent's own latest note (objective, report, receipt, AI summary). */
  status_line: string
  /** What is happening now, in plain words (server template). */
  summary: string
  /** What to do next, in plain words; empty when nothing is needed. */
  next_step: string
  /** Plain labels of `chips` (chips are stable codes, shown under Details). */
  labels: string[]
  /** Technical detail for the Details disclosure. */
  details: string[]
  next_actions: string[]
  age_ms: number
  age_source: string
  heat: BoardHeat
  ai: BoardAiNote | null
  actions: BoardAction[]
  rollup: boolean
  pending: boolean
  dead_count: number | null
}

export interface BoardTodo {
  id: string
  text: string
  kind: string
  urgency: TodoUrgency
  project_id: string | null
  sources: { kind: string; id: string }[]
  age_ms: number
}

export interface BoardCounts {
  doing: number
  needs_you: number
  stale_needs_work: number
  stale_likely_done: number
  close: number
  closed: number
}

export interface BoardProject {
  id: string
  name: string
  status_line: string
  /** The orchestrator's plain summary. */
  summary?: string
  orchestrator_column: BoardColumn
  counts: BoardCounts
}

export interface BoardModelSummary {
  enabled: boolean
  model_id?: string
  cached_items?: number
  sent_items?: number
  /** Ambiguous items left out because the cost caps fit only a prefix. */
  skipped_items?: number
  calls: number
  input_tokens: number
  output_tokens: number
  usd: number
  error: string | null
  skipped: boolean
  batches_done: number
  batches_total: number
}

export interface WorkBoard {
  revision: number
  generation_id: string | null
  state: 'empty' | 'idle' | 'running' | 'failed'
  stage: string | null
  generated_at_unix_ms: number | null
  fingerprint: string | null
  mode: 'deterministic' | 'model'
  model_available: boolean
  model?: BoardModelSummary | null
  model_estimate_usd?: number | null
  thresholds: {
    fresh_ms: number
    review_window_ms: number
    cold_ms: number
    /** The relevance window (main board vs Older work). */
    recent_ms?: number
    waiting_ms?: number
    suggest_archive_ms?: number
  }
  warnings: string[]
  error: string | null
  timings?: {
    deterministic_ms: number | null
    herdr_ms: number | null
    model_ms: number | null
    total_ms: number | null
  }
  items?: BoardItem[]
  /** Every to-do (uncapped); the web caps them with `capTodos`. */
  todos?: BoardTodo[]
  /** Worker records without a live Herdr agent: counted, never cards. */
  hidden?: { not_live: number; dead_runtimes: number }
  projects?: BoardProject[]
  counts?: BoardCounts
  /** Cards in Older work, and those a review would still offer. */
  older?: number
  to_review?: number
  /** Helper cards plus subagents. */
  helpers?: number
  helper_rows?: BoardHelperRow[]
}

/** A subagent shown under "Show helpers". */
export interface BoardHelperRow {
  id: string
  title: string
  parent_id: string | null
  parent_title: string | null
  status: string
  age_ms: number
}

export type WorkBoardResponse = WorkBoard | { unchanged: true; revision: number }

export function isUnchanged(
  response: WorkBoardResponse,
): response is { unchanged: true; revision: number } {
  return 'unchanged' in response && response.unchanged === true
}

export const BOARD_COLUMNS: { id: BoardColumn; label: string; hint: string }[] = [
  { id: 'doing', label: 'Working now', hint: 'Nothing for you to do. Each card says what it is on.' },
  {
    id: 'needs_you',
    label: 'Needs you',
    hint: 'Waiting on you: a question, a problem, or finished work to review.',
  },
  {
    id: 'stale',
    label: 'Gone quiet',
    hint: 'Open but quiet for a while.',
  },
  {
    id: 'close',
    label: 'Finished: ready to close',
    hint: 'Done. Review if you want, then close it.',
  },
]

export const CLOSED_COLUMN = { label: 'Closed', hint: 'Already closed. Shown for reference.' }

/** The "Gone quiet" groups; `no_task` is the web's own split of `needs_work`. */
export type StaleGroup = 'needs_work' | 'no_task' | 'likely_done'

export const SECTION_LABELS: Record<StaleGroup, { label: string; hint: string }> = {
  needs_work: { label: 'Stalled, not finished', hint: 'Quiet, and something is still open for these agents.' },
  no_task: {
    label: 'Open, no task',
    hint: 'Chats you were driving yourself, or agents Yard has no task for. Close them when you are done.',
  },
  likely_done: {
    label: 'Quiet, probably done',
    hint: 'Nothing left open and quiet for a week, or the AI summary says it is done. Check first, then close.',
  },
}

/** A chat with no Yard task: untracked by Yard, or a worker with no task. */
export function hasNoTask(item: Pick<BoardItem, 'chips'>): boolean {
  return item.chips.includes('unassigned') || item.chips.includes('loose agent')
}

/** One line at the top of the board: how to read it. */
export const HOW_TO_READ =
  'One line per open agent you worked with recently, most urgent first. \u201cNeeds you\u201d are your to-dos; Open goes to the agent. ' +
  'Click a line for its details. Quieter agents wait under \u201cQuiet\u201d and \u201cOlder\u201d.'

/** The heat key, kept in Details (the age is printed on every line). */
export const HEAT_KEY =
  'The dot before each line is its last activity: darkest = active in the last hour, then active today, quiet for days, quiet over a week.'

/** Separator between parts of one line (never " . ", which reads as a typo). */
export const SEP = ' \u00b7 '

export interface BoardFilter {
  projects: string[]
  loose: boolean
  text: string
  showClosed: boolean
}

export const EMPTY_FILTER: BoardFilter = {
  projects: [],
  loose: false,
  text: '',
  showClosed: false,
}

export function matchesFilter(item: BoardItem, filter: BoardFilter): boolean {
  if (item.column === 'closed' && !filter.showClosed) return false
  const scoped = filter.projects.length > 0 || filter.loose
  if (scoped) {
    const inProject =
      item.project_id !== null && filter.projects.includes(item.project_id)
    const loose = filter.loose && item.kind === 'loose_agent'
    if (!inProject && !loose) return false
  }
  const text = filter.text.trim().toLowerCase()
  if (text) {
    const haystack = [
      item.title,
      item.objective,
      item.status_line,
      item.summary ?? '',
      item.next_step ?? '',
      item.about ?? '',
      item.status ?? '',
      item.your_move ?? '',
      ...(item.labels ?? []),
      item.project_name ?? '',
      ...item.next_actions,
    ]
      .join(' ')
      .toLowerCase()
    if (!haystack.includes(text)) return false
  }
  return true
}

/** The "Needs you" groups: what to answer first, then what to review. */
export const NEED_LABELS: Record<BoardNeed, { label: string; hint: string }> = {
  answer: {
    label: 'Blocked or has a question',
    hint: 'It is waiting on you: a question, a blocker only you can clear, or a failure. Do these first.',
  },
  review: {
    label: 'Finished, review and close',
    hint: 'It says it is done or reported a result. Review it, then mark it done or close it.',
  },
}

export type SectionId = StaleGroup | BoardNeed

export interface GroupedColumn {
  id: BoardColumn
  label: string
  items: BoardItem[]
  sections?: { id: SectionId; label: string; items: BoardItem[] }[]
}

/** On the main board: not a helper and not in Older work. */
export function isOnMain(item: Pick<BoardItem, 'helper' | 'relevance'>): boolean {
  return !item.helper && item.relevance !== 'older'
}

/**
 * Older-work cards (not helpers, not closed): the ones flagged "suggest
 * archive" first, oldest first, then the rest, most recently active first.
 */
export function olderItems(items: BoardItem[], filter: BoardFilter): BoardItem[] {
  const older = items.filter(
    (item) =>
      !item.rollup &&
      !item.helper &&
      item.relevance === 'older' &&
      item.column !== 'closed' &&
      matchesFilter(item, { ...filter, showClosed: false }),
  )
  const archive = older
    .filter((item) => item.suggest_archive)
    .sort((left, right) => right.age_ms - left.age_ms || left.id.localeCompare(right.id))
  const quiet = older
    .filter((item) => !item.suggest_archive)
    .sort((left, right) => left.age_ms - right.age_ms || left.id.localeCompare(right.id))
  return [...archive, ...quiet]
}

/** What "Review older work" walks through: Older work not skipped since, in the Older-work order. */
export function reviewQueue(items: BoardItem[]): BoardItem[] {
  return olderItems(items, EMPTY_FILTER).filter((item) => !item.reviewed)
}

/** Older work that can be closed: flagged "suggest archive", or finished and ready to close. */
export function cleanupQueue(items: BoardItem[]): BoardItem[] {
  return olderItems(items, EMPTY_FILTER).filter(
    (item) => item.suggest_archive || item.column === 'close',
  )
}

/**
 * The one line under "Older work": "7 untouched 2+ weeks, 5 quiet." The
 * review walks them in that order (the untouched ones, suggested for
 * archive, first). Empty when there is no older work.
 */
export function olderLine(items: BoardItem[], thresholds: WorkBoard['thresholds'] | undefined): string {
  const older = olderItems(items, EMPTY_FILTER)
  if (older.length === 0) return ''
  const untouched = older.filter((item) => item.suggest_archive).length
  const days = Math.round((thresholds?.suggest_archive_ms ?? 14 * 86_400_000) / 86_400_000)
  const span = days % 7 === 0 ? `${days / 7}+ week${days === 7 ? '' : 's'}` : `${days}+ days`
  const parts = []
  if (untouched > 0) parts.push(`${untouched} untouched ${span}`)
  if (older.length > untouched) parts.push(`${older.length - untouched} quiet`)
  return `${parts.join(', ')}.`
}

/** The review banner for a card untouched past the threshold: "Untouched 20 days — suggest archive." */
export function archiveBanner(item: Pick<BoardItem, 'suggest_archive' | 'age_ms' | 'activity_known'>): string {
  if (!item.suggest_archive) return ''
  const days = Math.floor(item.age_ms / 86_400_000)
  return item.activity_known === false || days < 1
    ? 'Suggest archive.'
    : `Untouched ${days} day${days === 1 ? '' : 's'} \u2014 suggest archive.`
}

/** Helper cards (subagents come separately as `helper_rows`). */
export function helperItems(items: BoardItem[], filter: BoardFilter): BoardItem[] {
  return items.filter(
    (item) => !item.rollup && !!item.helper && item.column !== 'closed' && matchesFilter(item, filter),
  )
}

/** The card's "Status:" line: the status, then "2 hr ago" when the age is known. */
export function statusWithAge(
  item: Pick<BoardItem, 'status' | 'summary' | 'activity_known' | 'age_ms' | 'column'>,
): string {
  // An empty status is meaningful: nothing beyond the age is known.
  const status = item.status ?? item.summary ?? ''
  if (item.activity_known === false || item.column === 'doing') return status
  return status ? `${status}${SEP}${formatAgo(item.age_ms)}` : formatAgo(item.age_ms)
}

/** The routes the review's Done and Archive use for a card (existing dialogs only). */
export function reviewActions(item: Pick<BoardItem, 'actions'>): {
  done: BoardAction | null
  archive: BoardAction | null
} {
  const has = (action: BoardAction) => item.actions.includes(action)
  const done: BoardAction | null = has('mark_done') ? 'mark_done' : has('end_session') ? 'end_session' : null
  const archive: BoardAction | null =
    done !== 'end_session' && has('end_session')
      ? 'end_session'
      : has('hide')
        ? 'hide'
        : has('clean_up')
          ? 'clean_up'
          : null
  return { done, archive }
}

export type ReviewKey = 'keep' | 'done' | 'archive' | 'skip' | 'close'

/** Keyboard of the review: k / d / a / s, Escape closes. */
export function reviewKey(key: string): ReviewKey | null {
  switch (key) {
    case 'k':
    case 'K':
      return 'keep'
    case 'd':
    case 'D':
      return 'done'
    case 'a':
    case 'A':
      return 'archive'
    case 's':
    case 'S':
    case 'ArrowRight':
      return 'skip'
    case 'Escape':
      return 'close'
    default:
      return null
  }
}

/** "Recent: 3 days · waiting on you: 7 days · suggest archive: 14 days". */
export function windowLine(thresholds: WorkBoard['thresholds']): string {
  const days = (ms: number | undefined, fallback: number) => {
    const count = Math.round((ms ?? fallback * 86_400_000) / 86_400_000)
    return `${count} day${count === 1 ? '' : 's'}`
  }
  return [
    `Main board: activity in the last ${days(thresholds.recent_ms, 3)}`,
    `waiting on you in the last ${days(thresholds.waiting_ms, 7)}`,
    `or kept`,
    `Older work suggests archiving after ${days(thresholds.suggest_archive_ms, 14)}`,
  ].join(SEP)
}

/** Cards per column (orchestrator roll-ups, helpers and Older work excluded), in server order. */
export function groupItems(
  items: BoardItem[],
  filter: BoardFilter,
): GroupedColumn[] {
  const visible = items.filter(
    (item) => !item.rollup && isOnMain(item) && matchesFilter(item, filter),
  )
  const columns: GroupedColumn[] = BOARD_COLUMNS.map((column) => ({
    ...column,
    items: visible.filter((item) => item.column === column.id),
  }))
  const needs = columns.find((column) => column.id === 'needs_you')
  if (needs && needs.items.length > 0) {
    // Answer before review; the server already orders each group oldest first.
    const review = (item: BoardItem) => item.need === 'review'
    needs.sections = (['answer', 'review'] as const)
      .map((id) => ({
        id,
        label: NEED_LABELS[id].label,
        items: needs.items.filter((item) => (id === 'review' ? review(item) : !review(item))),
      }))
      .filter((section) => section.items.length > 0)
    needs.items = needs.sections.flatMap((section) => section.items)
  }
  const stale = columns.find((column) => column.id === 'stale')
  if (stale) {
    const open = stale.items.filter((item) => item.section !== 'likely_done')
    stale.sections = [
      {
        id: 'needs_work' as const,
        label: SECTION_LABELS.needs_work.label,
        items: open.filter((item) => !hasNoTask(item)),
      },
      {
        id: 'no_task' as const,
        label: SECTION_LABELS.no_task.label,
        items: open.filter(hasNoTask),
      },
      {
        id: 'likely_done' as const,
        label: SECTION_LABELS.likely_done.label,
        items: stale.items.filter((item) => item.section === 'likely_done'),
      },
    ].filter((section) => section.items.length > 0)
    // Keep the column's own order (the keyboard follows the sections).
    stale.items = stale.sections.flatMap((section) => section.items)
  }
  if (filter.showClosed) {
    columns.push({
      id: 'closed',
      label: CLOSED_COLUMN.label,
      items: visible.filter((item) => item.column === 'closed'),
    })
  }
  return columns
}

export function formatAge(ageMs: number): string {
  const minutes = Math.floor(ageMs / 60_000)
  if (minutes < 1) return 'now'
  if (minutes < 60) return `${minutes}m`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours}h`
  const days = Math.floor(hours / 24)
  if (days < 60) return `${days}d`
  return `${Math.floor(days / 30)}mo`
}

/** An age as people say it: "just now", "5 min ago", "2 hr ago", "3 days ago". */
export function formatAgo(ageMs: number): string {
  const minutes = Math.floor(ageMs / 60_000)
  if (minutes < 1) return 'just now'
  if (minutes < 60) return `${minutes} min ago`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours} hr ago`
  const days = Math.floor(hours / 24)
  if (days < 60) return `${days} day${days === 1 ? '' : 's'} ago`
  const months = Math.floor(days / 30)
  return `${months} month${months === 1 ? '' : 's'} ago`
}

/** A clock time as people write it: "2:48 PM", no leading zero. */
export function formatClock(unixMs: number): string {
  return new Date(unixMs).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
}

/** Where a card's age comes from (`age_source` codes), for its tooltip. */
export const AGE_SOURCE_LABELS: Record<string, string> = {
  'assignment updated': 'its task last changed',
  'worker updated': 'its agent record last changed',
  receipt: 'it left a report on its finished work',
  'orchestrator updated': 'the orchestrator last changed',
  'terminal output changed': 'its terminal output changed',
  'runtime status changed': 'its status changed',
  'unchanged since first seen': 'nothing changed since the board first saw it',
  'first seen now': 'the board saw it for the first time',
}

export function ageTooltip(item: Pick<BoardItem, 'age_ms' | 'age_source' | 'heat'>): string {
  const source = AGE_SOURCE_LABELS[item.age_source] ?? item.age_source
  return `Last activity ${formatAgo(item.age_ms)} (${source}), ${HEAT_LABELS[item.heat]}`
}

export function heatFor(ageMs: number, coldMs = 7 * 86_400_000): BoardHeat {
  if (ageMs < 3_600_000) return 'hot'
  if (ageMs < 86_400_000) return 'warm'
  if (ageMs < coldMs) return 'cool'
  return 'cold'
}

export const HEAT_LABELS: Record<BoardHeat, string> = {
  hot: 'active in the last hour',
  warm: 'active today',
  cool: 'quiet for days',
  cold: 'quiet over a week',
}

/** The `/workers` fingerprint the server stores on the board. */
export function workersFingerprint(workers: WorkerCandidate[]): string {
  let sequence = 0n
  let updated = 0
  for (const candidate of workers) {
    const raw = candidate.worker.runtime?.state_change_sequence
    if (raw !== undefined) {
      try {
        const value = BigInt(raw)
        if (value > sequence) sequence = value
      } catch {
        // Ignore malformed sequences.
      }
    }
    if (candidate.worker.updated_at_unix_ms > updated) {
      updated = candidate.worker.updated_at_unix_ms
    }
  }
  return `${sequence}-${updated}`
}

/**
 * Whether the board is older than the `/workers` data the app already polls.
 * Known gap: which agents are cards also depends on Herdr's live pane list,
 * which is not part of this fingerprint, so a pane that closes (or a loose
 * agent that opens) before Yard records a runtime change does not turn the
 * pill on until the next worker update; regenerate to pick it up.
 */
export function isOutOfDate(
  board: WorkBoard | null,
  workers: WorkerCandidate[] | null,
  locallyChanged: boolean,
): boolean {
  if (!board || board.state === 'empty' || !board.fingerprint) return false
  if (locallyChanged) return true
  if (!workers) return false
  return workersFingerprint(workers) !== board.fingerprint
}

// ---- Filter state in the URL hash: #board?p=alpha,beta&loose=1&q=x&closed=1

export function filterToHash(filter: BoardFilter): string {
  const params = new URLSearchParams()
  if (filter.projects.length) params.set('p', filter.projects.join(','))
  if (filter.loose) params.set('loose', '1')
  if (filter.text) params.set('q', filter.text)
  if (filter.showClosed) params.set('closed', '1')
  const query = params.toString()
  return query ? `#board?${query}` : '#board'
}

export function filterFromHash(hash: string): BoardFilter | null {
  if (!hash.startsWith('#board')) return null
  const query = hash.slice('#board'.length).replace(/^\?/, '')
  const params = new URLSearchParams(query)
  return {
    projects: (params.get('p') ?? '').split(',').filter(Boolean),
    loose: params.get('loose') === '1',
    text: params.get('q') ?? '',
    showClosed: params.get('closed') === '1',
  }
}

// ---- Keyboard map (only while the board has focus)

export type BoardCommand =
  | { kind: 'move'; delta: number }
  | { kind: 'toggle' }
  | { kind: 'open' }
  | { kind: 'project' }
  | { kind: 'done' }
  | { kind: 'resume' }
  | { kind: 'generate' }
  | { kind: 'filter' }
  | { kind: 'back' }
  | { kind: 'help' }

export function boardCommand(event: {
  key: string
  altKey?: boolean
  ctrlKey?: boolean
  metaKey?: boolean
}): BoardCommand | null {
  if (event.altKey || event.ctrlKey || event.metaKey) return null
  switch (event.key) {
    case 'j':
    case 'ArrowDown':
      return { kind: 'move', delta: 1 }
    case 'k':
    case 'ArrowUp':
      return { kind: 'move', delta: -1 }
    case ' ':
      return { kind: 'toggle' }
    case 'Enter':
      return { kind: 'open' }
    case 'p':
      return { kind: 'project' }
    case 'd':
      return { kind: 'done' }
    case 'r':
      return { kind: 'resume' }
    case 'g':
      return { kind: 'generate' }
    case '/':
      return { kind: 'filter' }
    case 'Escape':
      return { kind: 'back' }
    case '?':
      return { kind: 'help' }
    default:
      return null
  }
}

export const KEYBOARD_HELP: [string, string][] = [
  ['j / k', 'Next / previous agent'],
  ['Space', 'Show or hide its details'],
  ['Enter', 'Open terminal'],
  ['p', 'Open project'],
  ['d', 'Mark done (asks you to confirm)'],
  ['r', 'Resume'],
  ['g', 'Refresh board'],
  ['/', 'Filter'],
  ['Esc', 'Back'],
]

/** What a card's primary and secondary buttons do. */
export function cardButtons(item: BoardItem): BoardAction[] {
  return item.actions.slice(0, 2)
}

export const ACTION_LABELS: Record<BoardAction, string> = {
  open_terminal: 'Open terminal',
  open_project: 'Project page',
  mark_done: 'Mark done\u2026',
  resume: 'Resume\u2026',
  end_session: 'Close agent\u2026',
  archive_project: 'Archive project\u2026',
  clean_up: 'Clean up\u2026',
  hide: 'Archive\u2026',
}

/** The dialog an action opens; every write goes through one. */
export type ActionDialog =
  | 'navigate'
  | 'completion'
  | 'end-session'
  | 'archive-project'
  | 'hide-stale'
  | 'resume'

export function actionDialog(action: BoardAction): ActionDialog {
  switch (action) {
    case 'open_terminal':
    case 'open_project':
      return 'navigate'
    case 'mark_done':
      return 'completion'
    case 'end_session':
      return 'end-session'
    case 'archive_project':
      return 'archive-project'
    case 'clean_up':
    case 'hide':
      return 'hide-stale'
    case 'resume':
      return 'resume'
  }
}

/** The single list's groups (main board only; orchestrator roll-ups left out). */
export interface BoardList {
  /** Waiting on you (blocked or a question, then finished work to review), then finished and ready to close. */
  needs: BoardItem[]
  working: BoardItem[]
  /** Gone quiet: open steps, then no task, then probably done. */
  quiet: BoardItem[]
  /** Only with "Show closed cards". */
  closed: BoardItem[]
}

export function boardList(items: BoardItem[], filter: BoardFilter): BoardList {
  const columns = groupItems(items, filter)
  const of = (id: BoardColumn) => columns.find((column) => column.id === id)?.items ?? []
  return {
    needs: [...of('needs_you'), ...of('close')],
    working: of('doing'),
    quiet: of('stale'),
    closed: filter.showClosed ? of('closed') : [],
  }
}

/** Keep one sentence: "Blocked: X. Then Y." -> "Blocked: X." */
function firstSentence(text: string): string {
  const trimmed = text.trim()
  const match = /^.+?[.?!](?=\s+[A-Z\u201c"])/.exec(trimmed)
  return match ? match[0] : trimmed
}

/**
 * The one sentence after a row's title: what to do for a card that needs
 * you, what it is doing for one that is working, else its move (or, when
 * the move is "Nothing — …", its status).
 */
export function rowLine(item: BoardItem, group: 'needs' | 'working' | 'quiet' | 'closed' | 'helper'): string {
  const move = (item.your_move || item.next_step || '').trim()
  const status = (item.status ?? item.summary ?? '').trim()
  const nothing = !move || /^nothing\b/i.test(move)
  if (group === 'working') {
    const doing = status.replace(/^working:\s*/i, '')
    return firstSentence(doing ? doing[0].toUpperCase() + doing.slice(1) : 'Working')
  }
  if (group === 'needs' || (group === 'quiet' && !nothing)) return firstSentence(nothing ? status : move)
  return firstSentence(status || (nothing ? '' : move))
}

/**
 * The plain text of a card the app moved before the next generation (the
 * same wording as the server's templates).
 */
export const MOVED_TEXT = {
  closed: { summary: 'Closed, nothing left to do', next_step: '' },
  completed: { summary: 'Finished', next_step: 'Close it to free the terminal.' },
  question: {
    summary: 'Has a question for you',
    next_step: 'Open it and answer the question.',
  },
} as const

/**
 * The agent named in the middle of a to-do (as the server writes it): the
 * title in quotes; the project is the to-do group's heading.
 */
export function agentName(item: Pick<BoardItem, 'title'>): string {
  return `\u201c${item.title}\u201d`
}

/** Move a card after a successful action, before the next generation. */
export function moveItem(
  board: WorkBoard,
  itemId: string,
  column: BoardColumn,
): WorkBoard {
  return {
    ...board,
    items: (board.items ?? []).map((item) =>
      item.id === itemId
        ? {
            ...item,
            column,
            section: column === 'stale' ? item.section : null,
            ...(column === 'closed'
              ? MOVED_TEXT.closed
              : column === 'close'
                ? MOVED_TEXT.completed
                : {}),
          }
        : item,
    ),
  }
}

/**
 * The header line in plain words: when the board was made and how it was
 * sorted. Calls, tokens, cost and timings go to `headerDetails`.
 */
export function headerLine(board: WorkBoard, nowMs: number): string {
  if (board.state === 'empty' || board.generated_at_unix_ms === null) {
    return 'Not made yet. Press Refresh board to read your open agents.'
  }
  const time = formatClock(board.generated_at_unix_ms)
  const parts = [`Updated ${time} (${formatAgo(Math.max(0, nowMs - board.generated_at_unix_ms))})`]
  const model = board.model
  if (board.mode === 'model' && model) {
    if (model.skipped || model.error) {
      parts.push(model.error ? 'AI summaries did not run' : 'AI summaries unchanged since last time')
    } else if ((board.items ?? []).some((item) => item.ai)) {
      parts.push('With AI summaries')
    } else {
      parts.push('AI summaries ran but added nothing')
    }
  }
  if (model?.skipped_items) {
    parts.push(
      `${model.skipped_items} card${model.skipped_items === 1 ? '' : 's'} without an AI summary (cost limit)`,
    )
  }
  return parts.join(SEP)
}

/** The technical side of the header (for its Details tooltip). */
export function headerDetails(board: WorkBoard): string {
  if (board.state === 'empty' || board.generated_at_unix_ms === null) {
    return board.model_available ? 'Model pass available.' : 'Model pass not configured (deterministic only).'
  }
  const parts: string[] = []
  const model = board.model
  if (board.mode === 'model' && model) {
    if (model.skipped) {
      parts.push(
        model.error
          ? `deterministic (model refused: ${model.error})`
          : `model: no changes since last run (${model.cached_items ?? 0} cached)`,
      )
    } else {
      parts.push(
        [
          'deterministic + model',
          `${model.calls} call${model.calls === 1 ? '' : 's'}`,
          `${(model.input_tokens + model.output_tokens).toLocaleString()} tokens`,
          `$${model.usd.toFixed(3)}`,
        ].join(SEP),
      )
    }
    if (model.model_id) parts.push(`model ${model.model_id}`)
  } else {
    parts.push(board.model_available ? 'deterministic' : 'deterministic only (model pass not configured)')
  }
  const first = board.timings?.deterministic_ms
  if (typeof first === 'number') parts.push(`first view in ${first} ms`)
  if (board.generation_id) parts.push(`generation ${board.generation_id}`)
  parts.push(`revision ${board.revision}`)
  return parts.join(SEP)
}

/**
 * The announced generation state: changes only with the state, stage or
 * revision (never with the clock), so screen readers hear each step once.
 */
export function boardAnnouncement(board: WorkBoard | null): string {
  if (!board) return ''
  if (board.state === 'running') return 'Refreshing the work board'
  if (board.state === 'failed') return 'Work board refresh failed'
  if (board.state === 'empty') return ''
  const counts = board.counts
  if (!counts) return 'Work board ready'
  const needs = `${counts.needs_you} ${counts.needs_you === 1 ? 'needs' : 'need'} you`
  return `Work board ready: ${needs}, ${counts.doing} working now, ${counts.stale_needs_work + counts.stale_likely_done} gone quiet, ${counts.close} finished and ready to close`
}

/** Counts of the given items, as the server computes them (rollups excluded). */
export function countItems(items: BoardItem[]): BoardCounts {
  const counts: BoardCounts = {
    doing: 0,
    needs_you: 0,
    stale_needs_work: 0,
    stale_likely_done: 0,
    close: 0,
    closed: 0,
  }
  for (const item of items) {
    if (item.rollup || !isOnMain(item)) continue
    if (item.column === 'doing') counts.doing += 1
    else if (item.column === 'needs_you') counts.needs_you += 1
    else if (item.column === 'stale' && item.section === 'likely_done') counts.stale_likely_done += 1
    else if (item.column === 'stale') counts.stale_needs_work += 1
    else if (item.column === 'close') counts.close += 1
    else counts.closed += 1
  }
  return counts
}

/**
 * What the app knew when a board revision arrived. Only changes against
 * this baseline are reconciled, so a fresh board is never second-guessed.
 */
export interface ReconcileBaseline {
  revision: number
  lifecycles: Map<string, string>
  availability: Map<string, string>
  questions: Map<string, boolean>
}

export function newBaseline(revision: number): ReconcileBaseline {
  return { revision, lifecycles: new Map(), availability: new Map(), questions: new Map() }
}

/** True when `key` changed to a value other than its first-seen one. */
function changedSince<T>(seen: Map<string, T> | undefined, key: string, value: T | undefined) {
  if (value === undefined) return false
  if (!seen) return true
  if (!seen.has(key)) {
    seen.set(key, value)
    return false
  }
  return seen.get(key) !== value
}

/**
 * Reflect lifecycle changes the app learned after the board was generated
 * (an assignment completed or cancelled, a worker ended, a new agent
 * question) onto the cached board, so a card moves right after its action
 * succeeds. With a baseline, only changes since that revision count (the
 * baseline records first-seen values as it goes); counts are recomputed and
 * a question move adds a now "Reply" to-do. Returns the same object when
 * nothing changed.
 */
export function reconcileBoard(
  board: WorkBoard,
  assignments: { id: string; lifecycle: string }[],
  workers: WorkerCandidate[],
  hasQuestion: (item: BoardItem) => boolean = () => false,
  baseline?: ReconcileBaseline,
): WorkBoard {
  if (!board.items) return board
  const lifecycles = new Map(assignments.map((assignment) => [assignment.id, assignment.lifecycle]))
  const availability = new Map(
    workers.map((candidate) => [candidate.worker.id, candidate.availability]),
  )
  let changed = false
  const replies: BoardTodo[] = []
  const items = board.items.map((item) => {
    if (item.column === 'closed') return item
    const lifecycle = item.assignment_id ? lifecycles.get(item.assignment_id) : undefined
    const worker = item.worker_id ? availability.get(item.worker_id) : undefined
    const lifecycleMoved =
      item.assignment_id !== null &&
      changedSince(baseline?.lifecycles, item.assignment_id, lifecycle)
    const workerMoved =
      item.worker_id !== null && changedSince(baseline?.availability, item.worker_id, worker)
    const asked = hasQuestion(item)
    const questionMoved = changedSince(baseline?.questions, item.id, asked) && asked
    let next: BoardItem = item
    if ((workerMoved && worker === 'ended') || (lifecycleMoved && lifecycle === 'cancelled')) {
      next = { ...item, column: 'closed', section: null, ...MOVED_TEXT.closed }
    } else if (lifecycleMoved && lifecycle === 'completed' && item.column !== 'close') {
      next = {
        ...item,
        column: 'close',
        section: null,
        actions: ['end_session', 'open_project'],
        ...MOVED_TEXT.completed,
        labels: [],
      }
    } else if (questionMoved && item.column !== 'needs_you' && !item.chips.includes('question')) {
      next = {
        ...item,
        column: 'needs_you',
        section: null,
        rollup: false,
        chips: [...item.chips, 'question'],
        // The summary says it; a chip would repeat it.
        labels: (item.labels ?? []).filter((label) => label !== 'has a question'),
        ...MOVED_TEXT.question,
      }
      replies.push({
        id: `reply-${item.id}`,
        text: `Answer the question from ${agentName(item)}`,
        kind: 'reply',
        urgency: 'now',
        project_id: item.project_id,
        sources: [{ kind: 'item', id: item.id }],
        age_ms: 0,
      })
    }
    if (next !== item) changed = true
    return next
  })
  if (!changed) return board
  const todos = [
    ...replies.filter((reply) => !(board.todos ?? []).some((todo) => todo.id === reply.id)),
    ...(board.todos ?? []),
  ]
  const projects = board.projects?.map((project) => ({
    ...project,
    counts: countItems(items.filter((item) => item.project_id === project.id)),
  }))
  return { ...board, items, todos, counts: countItems(items), projects }
}

/** An open step that is really the agent asking the user something. */
export function isQuestion(step: string): boolean {
  return step.trim().endsWith('?')
}

/** The open steps worth listing on a card: not the one "You:" quotes. */
export function cardSteps(item: Pick<BoardItem, 'next_actions' | 'next_step'>): string[] {
  const next = (item.next_step ?? '').toLowerCase()
  return item.next_actions
    .filter((step) => !next.includes(step.trim().replace(/\.$/, '').toLowerCase()))
    .slice(0, 2)
}

/** The out-of-date pill: why it is stale, with the one refresh verb. */
export function outOfDateLabel(board: Pick<WorkBoard, 'generated_at_unix_ms'> | null): string {
  const since = board?.generated_at_unix_ms
  return since ? `New activity since ${formatClock(since)}${SEP}Refresh board` : `New activity${SEP}Refresh board`
}

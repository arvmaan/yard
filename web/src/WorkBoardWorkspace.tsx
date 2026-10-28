import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from 'react'
import {
  ChevronDown,
  ClipboardList,
  RefreshCw,
  Sparkles,
  X,
} from 'lucide-react'
import {
  ACTION_LABELS,
  BOARD_COLUMNS,
  HEAT_KEY,
  HEAT_LABELS,
  HOW_TO_READ,
  outOfDateLabel,
  ageTooltip,
  KEYBOARD_HELP,
  boardAnnouncement,
  boardCommand,
  cardButtons,
  cardSteps,
  isQuestion,
  SEP,
  formatAgo,
  headerDetails,
  headerLine,
  helperItems,
  olderItems,
  reviewActions,
  reviewKey,
  reviewQueue,
  archiveBanner,
  boardList,
  EMPTY_FILTER,
  rowLine,
  olderLine,
  statusWithAge,
  windowLine,
  type BoardAction,
  type BoardFilter,
  type BoardItem,
  type WorkBoard,
} from './workBoard'
import './WorkBoard.css'

export interface WorkBoardWorkspaceProps {
  board: WorkBoard | null
  error: string | null
  filter: BoardFilter
  generateError: string | null
  nowMs: number
  onAction: (item: BoardItem, action: BoardAction, trigger: HTMLElement) => void
  /** The existing cleanup dialog for worker records that are not cards. */
  onCleanUp?: (trigger: HTMLElement) => void
  onClose: () => void
  onFilterChange: (filter: BoardFilter) => void
  onGenerate: (useModel: boolean) => void
  /** Keep (pin), Skip (reviewed until new activity) or Clear one card. */
  onTriage?: (itemId: string, choice: 'keep' | 'skip' | 'clear') => void
  outOfDate: boolean
  returnFocus: HTMLElement | null
}

const KIND_LABELS: Record<BoardItem['kind'], string> = {
  assignment: 'Agent with a task',
  worker: 'Agent',
  loose_agent: 'Terminal agent outside Yard (seen by Herdr only)',
  dead_aggregate: 'Old agent records',
  orchestrator: 'Project orchestrator',
}

const RELEVANCE_LABELS: Record<NonNullable<BoardItem['relevance']>, string> = {
  recent: 'Recent activity',
  waiting: 'Waiting on you',
  pinned: 'Kept by you',
  older: 'Older work',
}

/** Technical detail, kept off the face of the card. */
function detailRows(item: BoardItem): [string, string][] {
  const rows: [string, string][] = [
    ['Kind', KIND_LABELS[item.kind] ?? item.kind],
    ['Board rule', `${item.rule}${item.section ? ` (${item.section})` : ''}`],
  ]
  if (item.chips.length > 0) rows.push(['Signals', item.chips.join(', ')])
  rows.push([
    'Age from',
    item.activity_known === false
      ? `${item.age_source} (no usable age)`
      : `${item.age_source} (${Math.round(item.age_ms / 60_000)} min)`,
  ])
  if (item.title_source) rows.push(['Title from', item.title_source])
  if (item.relevance) rows.push(['On the board', RELEVANCE_LABELS[item.relevance]])
  if (item.helper) rows.push(['Helper', item.helper])
  if (item.summary) rows.push(['Summary', item.summary])
  if (item.objective) rows.push(['Task', item.objective])
  if (item.status_line && item.status_line !== item.objective) rows.push(['Latest', item.status_line])
  for (const step of cardSteps(item)) rows.push([isQuestion(step) ? 'Its question' : 'Its next step', step])
  if (item.ai) {
    rows.push([
      'AI',
      `${item.ai.verdict.replace(/_/g, ' ')}, confidence ${Math.round(item.ai.confidence * 100)}% (from ${item.ai.evidence})`,
    ])
    if (item.ai.quote) rows.push(['AI quoted', `"${item.ai.quote}"`])
  }
  for (const line of item.details ?? []) rows.push(['Yard', line])
  rows.push(['Card id', item.id])
  if (item.worker_id) rows.push(['Worker id', item.worker_id])
  if (item.assignment_id) rows.push(['Task id', item.assignment_id])
  return rows
}

type RowGroup = 'needs' | 'working' | 'quiet' | 'closed' | 'helper'

/**
 * One open agent as one line: title, its project (small, muted), one
 * sentence (what to do, or what it is doing) and its age. Clicking the line
 * shows the rest (About, Status, Your move, labels, buttons, Details).
 */
function Row({
  item,
  group,
  focused,
  open,
  onAction,
  onFocus,
  onToggle,
  onTriage,
}: {
  item: BoardItem
  group: RowGroup
  focused: boolean
  open: boolean
  onAction: WorkBoardWorkspaceProps['onAction']
  onFocus: () => void
  onToggle: () => void
  onTriage?: WorkBoardWorkspaceProps['onTriage']
}) {
  const line = rowLine(item, group)
  const knownAge = item.activity_known !== false
  const primary = item.actions.includes('open_terminal') ? 'open_terminal' : item.actions[0]
  return (
    <li
      aria-label={`${item.title}${item.project_name ? ` (${item.project_name})` : ''}${line ? `: ${line}` : ''}`}
      className="board-row"
      data-group={group}
      data-heat={knownAge ? item.heat : undefined}
      data-item-id={item.id}
      data-open={open || undefined}
      data-pending={item.pending || undefined}
      onFocus={onFocus}
      tabIndex={focused ? 0 : -1}
    >
      <div className="board-row__line">
        <span aria-hidden="true" className="board-row__heat" title={knownAge ? ageTooltip(item) : undefined} />
        <button
          aria-expanded={open}
          className="board-row__face"
          onClick={onToggle}
          tabIndex={-1}
          title={open ? 'Hide details' : 'Show details'}
          type="button"
        >
          <strong className="board-row__title">{item.title}</strong>
          {item.project_name ? <span className="board-row__project">{item.project_name}</span> : null}
          {line ? <span className="board-row__text">{`— ${line}`}</span> : null}
        </button>
        {knownAge ? (
          <span className="board-row__age" title={ageTooltip(item)}>
            {formatAgo(item.age_ms)}
          </span>
        ) : (
          <span className="board-row__age" />
        )}
        {group === 'needs' && primary ? (
          <button
            className="board-button board-button--primary board-row__open"
            onClick={(event) => onAction(item, primary, event.currentTarget)}
            tabIndex={focused ? 0 : -1}
            title={ACTION_LABELS[primary]}
            type="button"
          >
            Open
          </button>
        ) : null}
      </div>
      {open ? <RowMore item={item} onAction={onAction} onTriage={onTriage} /> : null}
    </li>
  )
}

/** Everything that is not on a row's face. */
function RowMore({
  item,
  onAction,
  onTriage,
}: {
  item: BoardItem
  onAction: WorkBoardWorkspaceProps['onAction']
  onTriage?: WorkBoardWorkspaceProps['onTriage']
}) {
  const buttons = cardButtons(item)
  const labels = item.labels ?? []
  const unsure = item.chips.includes('?')
  // No About line when nothing beyond the title is known (never a placeholder).
  const about = item.about ?? ''
  const status = statusWithAge(item)
  const yourMove = item.your_move || item.next_step || ''
  const chips = [
    item.kind === 'loose_agent' ? 'Outside Yard' : '',
    item.pinned ? 'kept' : '',
    item.suggest_archive && item.relevance === 'older' ? 'suggest archive' : '',
    ...labels,
    item.ai && (item.ai.about || item.ai.status || item.ai.your_move || item.ai.verdict) ? 'AI summary' : '',
    unsure ? 'not sure' : '',
  ].filter(Boolean)
  return (
    <div className="board-row__more">
      <dl className="board-row__lines">
        {about ? (
          <div>
            <dt>About:</dt>
            <dd>{about}</dd>
          </div>
        ) : null}
        <div>
          <dt>Status:</dt>
          <dd>{status}</dd>
        </div>
        {yourMove ? (
          <div>
            <dt>Your move:</dt>
            <dd>{yourMove}</dd>
          </div>
        ) : null}
      </dl>
      {chips.length > 0 ? <p className="board-row__labels">{chips.join(SEP)}</p> : null}
      {buttons.length > 0 || onTriage ? (
        <div className="board-row__actions">
          {buttons.map((action, index) => (
            <button
              className={index === 0 ? 'board-button board-button--primary' : 'board-button'}
              data-action={action}
              key={action}
              title={action === 'hide' ? 'Archive this chat: it leaves the board. The terminal stays open; nothing is closed.' : undefined}
              onClick={(event) => onAction(item, action, event.currentTarget)}
              type="button"
            >
              {ACTION_LABELS[action]}
            </button>
          ))}
          {onTriage && (item.pinned || item.relevance === 'older') ? (
            <button
              className="board-button"
              data-action={item.pinned ? 'unpin' : 'keep'}
              title={item.pinned ? 'Let it move back to Older work when it is quiet' : 'Keep it on the main board (K in the review)'}
              onClick={() => onTriage(item.id, item.pinned ? 'clear' : 'keep')}
              type="button"
            >
              {item.pinned ? 'Stop keeping' : 'Keep on board'}
            </button>
          ) : null}
        </div>
      ) : null}
      <details className="board-row__details">
        <summary>Details</summary>
        <dl>
          {detailRows(item).map(([term, value], index) => (
            <div key={`${term}-${index}`}>
              <dt>{term}</dt>
              <dd>{value}</dd>
            </div>
          ))}
        </dl>
      </details>
    </div>
  )
}

/**
 * "Review older work": one Older-work card at a time. Keep pins it to the
 * board, Done and Archive open the card's existing confirm dialogs, Skip
 * leaves it in Older work and does not ask again until it changes. The
 * queue is fixed when the review opens, so cards that move do not reshuffle
 * it.
 */
function ReviewOlderWork({
  items,
  onAction,
  onClose,
  onTriage,
  start = 0,
}: {
  items: BoardItem[]
  onAction: WorkBoardWorkspaceProps['onAction']
  onClose: () => void
  onTriage: NonNullable<WorkBoardWorkspaceProps['onTriage']>
  /** The card to open first (a row clicked in Older work). */
  start?: number
}) {
  const titleId = useId()
  const panel = useRef<HTMLDivElement>(null)
  const [queue] = useState(() => items)
  const [index, setIndex] = useState(() => Math.min(Math.max(start, 0), Math.max(items.length - 1, 0)))
  const item = queue[index]
  useEffect(() => {
    const frame = window.requestAnimationFrame(() => panel.current?.focus())
    return () => window.cancelAnimationFrame(frame)
  }, [index])
  const next = () => {
    if (index + 1 >= queue.length) onClose()
    else setIndex(index + 1)
  }
  const actions = item ? reviewActions(item) : { done: null, archive: null }
  // "Done" means nothing for a chat Yard does not track: only Archive.
  const done = item?.kind === 'loose_agent' ? null : actions.done
  const archive = actions.archive
  // The suggested action leads: Archive when the card is suggested for it.
  const archiveFirst = !!item?.suggest_archive && !!archive
  const run = (choice: 'keep' | 'done' | 'archive' | 'skip', trigger: HTMLElement | null) => {
    if (!item) return
    if (choice === 'keep' || choice === 'skip') {
      onTriage(item.id, choice)
      next()
      return
    }
    const action = choice === 'done' ? done : archive
    if (!action) return
    onAction(item, action, trigger ?? panel.current ?? document.body)
    next()
  }
  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    event.stopPropagation()
    if (event.key === 'Tab') {
      const focusable = Array.from(
        panel.current?.querySelectorAll<HTMLElement>('button:not([disabled])') ?? [],
      )
      if (focusable.length === 0) return
      const at = focusable.indexOf(document.activeElement as HTMLElement)
      const to = event.shiftKey
        ? at <= 0
          ? focusable.length - 1
          : at - 1
        : at < 0 || at >= focusable.length - 1
          ? 0
          : at + 1
      event.preventDefault()
      focusable[to]?.focus()
      return
    }
    if (event.metaKey || event.ctrlKey || event.altKey) return
    const command = reviewKey(event.key)
    if (!command) return
    event.preventDefault()
    if (command === 'close') onClose()
    else run(command, panel.current?.querySelector<HTMLElement>(`[data-review="${command}"]`) ?? null)
  }
  if (!item) return null
  return (
    <div className="board-review-backdrop" role="presentation">
      <div
        aria-labelledby={titleId}
        aria-modal="true"
        className="board-review"
        data-testid="board-review"
        onKeyDown={onKeyDown}
        ref={panel}
        role="dialog"
        tabIndex={-1}
      >
        <header className="board-review__head">
          <h3 id={titleId}>Review older work</h3>
          <span aria-live="polite" className="board-review__progress">
            {index + 1} of {queue.length}
          </span>
          <button aria-label="Close review" className="board-button board-button--quiet" onClick={onClose} type="button">
            <X aria-hidden="true" size={16} />
          </button>
        </header>
        {item.suggest_archive ? <p className="board-review__suggest">{archiveBanner(item)}</p> : null}
        <h4 className="board-review__title">{item.title}</h4>
        <p className="board-muted">{item.project_name ?? (item.kind === 'loose_agent' ? 'Outside Yard' : '')}</p>
        <dl className="board-card__lines">
          {item.about ? (
            <div>
              <dt>About:</dt>
              <dd>{item.about}</dd>
            </div>
          ) : null}
          <div>
            <dt>Status:</dt>
            <dd>{statusWithAge(item)}</dd>
          </div>
          <div className="board-card__move">
            <dt>Your move:</dt>
            <dd>{item.your_move || item.next_step}</dd>
          </div>
        </dl>
        <div className="board-review__actions">
          {archiveFirst ? (
            <button
              className="board-button board-button--primary"
              data-review="archive"
              onClick={(event) => run('archive', event.currentTarget)}
              title={archive ? `${ACTION_LABELS[archive]} (asks first)` : undefined}
              type="button"
            >
              Archive&hellip; <kbd>A</kbd>
            </button>
          ) : null}
          <button
            className={archiveFirst ? 'board-button' : 'board-button board-button--primary'}
            data-review="keep"
            onClick={(event) => run('keep', event.currentTarget)}
            type="button"
          >
            Keep <kbd>K</kbd>
          </button>
          {item.kind === 'loose_agent' ? null : (
            <button
              className="board-button"
              data-review="done"
              disabled={!done}
              onClick={(event) => run('done', event.currentTarget)}
              title={done ? `${ACTION_LABELS[done]} (asks first)` : 'Nothing to mark done for this card'}
              type="button"
            >
              Done&hellip; <kbd>D</kbd>
            </button>
          )}
          {archiveFirst ? null : (
            <button
              className="board-button"
              data-review="archive"
              disabled={!archive}
              onClick={(event) => run('archive', event.currentTarget)}
              title={archive ? `${ACTION_LABELS[archive]} (asks first)` : 'Nothing to archive for this card'}
              type="button"
            >
              Archive&hellip; <kbd>A</kbd>
            </button>
          )}
          <button className="board-button board-button--quiet" data-review="skip" onClick={(event) => run('skip', event.currentTarget)} type="button">
            Skip <kbd>S</kbd>
          </button>
        </div>
        <p className="board-muted board-review__hint">
          Keep puts it back on the board. {item.kind === 'loose_agent' ? 'Archive asks' : 'Done and Archive ask'} before
          {' '}{item.kind === 'loose_agent' ? 'it changes' : 'they change'} anything. Skip leaves it in
          Older work and won&rsquo;t ask again until it has new activity. Esc closes.
        </p>
      </div>
    </div>
  )
}

export function WorkBoardWorkspace(props: WorkBoardWorkspaceProps) {
  const {
    board,
    error,
    filter,
    generateError,
    nowMs,
    onAction,
    onCleanUp,
    onClose,
    onFilterChange,
    onGenerate,
    onTriage,
    outOfDate,
    returnFocus,
  } = props
  const titleId = useId()
  const workspace = useRef<HTMLElement>(null)
  const filterInput = useRef<HTMLInputElement>(null)
  const generateButton = useRef<HTMLButtonElement>(null)
  const [menuOpen, setMenuOpen] = useState(false)
  const [helpOpen, setHelpOpen] = useState(false)
  // Roving focus: the index of the one row in the tab order.
  const [focus, setFocus] = useState(0)
  // Rows showing their details.
  const [opened, setOpened] = useState<ReadonlySet<string>>(() => new Set())
  const [quietOpen, setQuietOpen] = useState(false)
  const [showHelpers, setShowHelpers] = useState(false)
  const [reviewing, setReviewing] = useState(false)
  const returnFocusRef = useRef(returnFocus)
  returnFocusRef.current = returnFocus

  const list = useMemo(() => boardList(board?.items ?? [], filter), [board?.items, filter])
  const older = useMemo(() => olderItems(board?.items ?? [], filter), [board?.items, filter])
  const helpers = useMemo(() => helperItems(board?.items ?? [], filter), [board?.items, filter])
  const toReview = useMemo(() => reviewQueue(board?.items ?? []), [board?.items])
  const olderSummary = useMemo(
    () => olderLine(board?.items ?? [], board?.thresholds),
    [board?.items, board?.thresholds],
  )
  // Every rendered row, in screen order (the keyboard follows exactly these).
  const rows = useMemo(() => {
    const out: { item: BoardItem; group: 'needs' | 'working' | 'quiet' | 'closed' | 'helper' }[] = []
    for (const item of list.needs) out.push({ item, group: 'needs' })
    for (const item of list.working) out.push({ item, group: 'working' })
    if (quietOpen) for (const item of list.quiet) out.push({ item, group: 'quiet' })
    for (const item of list.closed) out.push({ item, group: 'closed' })
    if (showHelpers) for (const item of helpers) out.push({ item, group: 'helper' })
    return out
  }, [list, quietOpen, showHelpers, helpers])
  const rowIndex = useMemo(() => new Map(rows.map((row, index) => [row.item.id, index])), [rows])
  useEffect(() => {
    setFocus((current) => Math.max(0, Math.min(current, rows.length - 1)))
  }, [rows])
  // Where the review starts, in its queue.
  const [reviewStart, setReviewStart] = useState(0)
  const running = board?.state === 'running'
  const empty = !board || board.state === 'empty'

  const openedEmpty = useRef(empty)
  useEffect(() => {
    const frame = window.requestAnimationFrame(() => {
      if (openedEmpty.current) generateButton.current?.focus()
      else workspace.current?.focus()
    })
    return () => window.cancelAnimationFrame(frame)
  }, [])

  useEffect(() => {
    const background = document.querySelector<HTMLElement>('.app-shell')
    if (!background) return
    const hadInert = background.hasAttribute('inert')
    const previousAriaHidden = background.getAttribute('aria-hidden')
    background.inert = true
    background.setAttribute('aria-hidden', 'true')
    return () => {
      if (!hadInert) background.inert = false
      if (previousAriaHidden === null) background.removeAttribute('aria-hidden')
      else background.setAttribute('aria-hidden', previousAriaHidden)
    }
  }, [])

  // A control that unmounts or is replaced (the empty state's Generate, the
  // model menu) drops focus to <body>, outside this dialog's key handler:
  // bring it back to the workspace.
  useEffect(() => {
    const frame = window.requestAnimationFrame(() => {
      const active = document.activeElement
      if (!active || active === document.body) workspace.current?.focus()
    })
    return () => window.cancelAnimationFrame(frame)
  }, [board?.state, board?.revision, menuOpen, empty])

  const close = () => {
    const target = returnFocusRef.current
    onClose()
    window.setTimeout(() => target?.focus(), 0)
  }

  const focusRow = (index: number) => {
    if (rows.length === 0) return
    const clamped = Math.max(0, Math.min(rows.length - 1, index))
    setFocus(clamped)
    const id = rows[clamped]?.item.id ?? ''
    window.requestAnimationFrame(() => {
      workspace.current?.querySelector<HTMLElement>(`.board-row[data-item-id="${CSS.escape(id)}"]`)?.focus()
    })
  }

  const toggleRow = (id: string) =>
    setOpened((current) => {
      const next = new Set(current)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })

  const focusedItem = (): BoardItem | null => {
    const active = document.activeElement
    const row = active?.closest<HTMLElement>('[data-item-id]')
    const id = row?.dataset.itemId
    return board?.items?.find((item) => item.id === id) ?? null
  }

  const runAction = (action: BoardAction) => {
    const item = focusedItem()
    if (!item || !item.actions.includes(action)) return
    const row = workspace.current?.querySelector<HTMLElement>(
      `[data-item-id="${CSS.escape(item.id)}"]`,
    )
    if (row) onAction(item, action, row)
  }

  const handleKeyDown = (event: ReactKeyboardEvent<HTMLElement>) => {
    if (reviewing) return
    const target = event.target as HTMLElement
    const typing =
      target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement
    if (event.key === 'Tab') {
      trapTab(event)
      return
    }
    if (typing) {
      if (event.key === 'Escape') {
        event.preventDefault()
        target.blur()
        workspace.current?.focus()
      }
      return
    }
    const command = boardCommand(event)
    if (!command) return
    const onRow = target.matches('.board-row')
    switch (command.kind) {
      case 'move':
        event.preventDefault()
        if (!target.closest('.board-row')) focusRow(0)
        else focusRow(focus + command.delta)
        break
      case 'toggle': {
        if (!onRow) return
        event.preventDefault()
        const id = target.dataset.itemId
        if (id) toggleRow(id)
        break
      }
      case 'open': {
        // Enter on a button, link or field is that control's own click.
        if (target.closest('button, a, input, select, textarea, summary')) return
        // Only the row itself opens its terminal.
        if (!onRow) return
        const item = focusedItem()
        if (!item) return
        const primary = item.actions[0]
        if (!primary) return
        event.preventDefault()
        runAction(item.actions.includes('open_terminal') ? 'open_terminal' : primary)
        break
      }
      case 'project':
        runAction('open_project')
        break
      case 'done':
        runAction('mark_done')
        break
      case 'resume':
        runAction('resume')
        break
      case 'generate':
        if (!running) onGenerate(false)
        break
      case 'filter':
        event.preventDefault()
        filterInput.current?.focus()
        break
      case 'back':
        event.preventDefault()
        if (helpOpen) setHelpOpen(false)
        else if (menuOpen) setMenuOpen(false)
        else close()
        break
      case 'help':
        setHelpOpen((open) => !open)
        break
    }
  }

  const trapTab = (event: ReactKeyboardEvent<HTMLElement>) => {
    if (!workspace.current) return
    const focusable = Array.from(
      workspace.current.querySelectorAll<HTMLElement>(
        'button:not([disabled]), input:not([disabled]), [tabindex]:not([tabindex="-1"])',
      ),
    ).filter((element) => element.getClientRects().length > 0)
    const first = focusable[0]
    const last = focusable.at(-1)
    if (!first || !last) return
    const active = document.activeElement
    if (event.shiftKey && (active === first || active === workspace.current)) {
      event.preventDefault()
      last.focus()
    } else if (!event.shiftKey && active === last) {
      event.preventDefault()
      first.focus()
    }
  }

  const renderRows = (group: (typeof rows)[number]['group'], items: BoardItem[], label: string) => (
    <ul aria-label={label} className="board-rows">
      {items.map((item) => {
        const index = rowIndex.get(item.id) ?? -1
        return (
          <Row
            focused={index === focus}
            group={group}
            item={item}
            key={item.id}
            onAction={onAction}
            onFocus={() => {
              if (index >= 0) setFocus(index)
            }}
            onToggle={() => toggleRow(item.id)}
            onTriage={onTriage}
            open={opened.has(item.id)}
          />
        )
      })}
    </ul>
  )

  const model = board?.model
  // Text depends only on the state and counts (never on the clock), so the
  // live region speaks once per change.
  const announcement = boardAnnouncement(board)
  const progress =
    running && model && model.batches_total > 0
      ? `Writing AI summaries (${model.batches_done} of ${model.batches_total} done)`
      : running
        ? board?.stage === 'reports'
          ? 'Reading what each orchestrator last reported'
          : board?.stage === 'tails'
            ? 'Reading recent terminal output'
            : board?.stage === 'model'
              ? 'Writing AI summaries'
              : 'Finding your open agents'
        : null

  return (
    <div className="modal-backdrop board-backdrop" role="presentation">
      <section
        aria-labelledby={titleId}
        aria-modal="true"
        className="board-workspace"
        onKeyDown={handleKeyDown}
        ref={workspace}
        role="dialog"
        tabIndex={-1}
      >
        <header className="board-header">
          <span className="board-header__icon">
            <ClipboardList aria-hidden="true" size={20} />
          </span>
          <div className="board-header__title">
            <p className="eyebrow">Your open agents at a glance</p>
            <h2 id={titleId}>Work board</h2>
          </div>
          <div className="board-generate">
            <button
              aria-busy={running}
              aria-disabled={running}
              className="board-button board-button--primary"
              onClick={() => {
                if (!running) onGenerate(false)
              }}
              ref={generateButton}
              type="button"
            >
              <RefreshCw aria-hidden="true" size={14} />
              {running ? 'Refreshing\u2026' : 'Refresh board'}
            </button>
            {board?.model_available ? (
              <>
                <button
                  aria-expanded={menuOpen}
                  aria-haspopup="menu"
                  aria-label="More refresh options"
                  aria-disabled={running}
                  className="board-button board-button--menu"
                  onClick={() => {
                    if (!running) setMenuOpen((open) => !open)
                  }}
                  type="button"
                >
                  <ChevronDown aria-hidden="true" size={14} />
                </button>
                {menuOpen ? (
                  <div className="board-menu" role="menu">
                    <button
                      onClick={() => {
                        setMenuOpen(false)
                        generateButton.current?.focus()
                        onGenerate(false)
                      }}
                      role="menuitem"
                      type="button"
                    >
                      Quick (no AI summaries)
                    </button>
                    <button
                      onClick={() => {
                        setMenuOpen(false)
                        generateButton.current?.focus()
                        onGenerate(true)
                      }}
                      role="menuitem"
                      type="button"
                    >
                      <Sparkles aria-hidden="true" size={13} /> With AI summaries
                      {typeof board.model_estimate_usd === 'number'
                        ? ` (costs at most $${board.model_estimate_usd.toFixed(3)})`
                        : ''}
                    </button>
                  </div>
                ) : null}
              </>
            ) : null}
          </div>
          <button
            aria-label="Close Work board"
            className="icon-button"
            onClick={close}
            title="Close Work board"
            type="button"
          >
            <X aria-hidden="true" size={18} />
          </button>
          <div className="board-header__line">
            <span>{board ? headerLine(board, nowMs) : 'Loading\u2026'}</span>
            {board ? (
              <details className="board-header__details">
                <summary>Details</summary>
                <span>{headerDetails(board)}</span>
                {model?.error ? <span>AI error: {model.error}</span> : null}
                <p className="board-heat-key">
                  {HEAT_KEY}{' '}
                  {(['hot', 'warm', 'cool', 'cold'] as const).map((heat) => (
                    <span key={heat}>
                      <i data-heat={heat} /> {HEAT_LABELS[heat]}{' '}
                    </span>
                  ))}
                </p>
                <p className="board-heat-key" data-testid="board-window">
                  {windowLine(board.thresholds)}
                </p>
                <p className="board-heat-key">
                  {`A report counts as current for ${Math.round((board.thresholds.fresh_ms ?? 900_000) / 60_000)} min; finished work waits ${Math.round((board.thresholds.review_window_ms ?? 86_400_000) / 3_600_000)} hr for review; after ${Math.round((board.thresholds.cold_ms ?? 604_800_000) / 86_400_000)} days quiet with nothing open a card is \u201cprobably finished\u201d.`}
                </p>
              </details>
            ) : null}
          </div>
          <p className="board-howto">
            <strong>How to read this board: </strong>
            {HOW_TO_READ}
          </p>
          <p aria-live="polite" className="board-visually-hidden">
            {announcement}
          </p>
        </header>
        {/* Always mounted, so a change of its text is announced. */}
        <div className="board-progress" data-active={progress ? true : undefined} role="status">
          {progress ? <span className="board-progress__bar" /> : null}
          <span>{progress ?? ''}</span>
        </div>
        {error || generateError || board?.error || model?.error ? (
          <div className="board-banner" role="alert">
            {generateError ??
              error ??
              board?.error ?? (
                <span title={model?.error ?? undefined}>
                  AI summaries aren{'\u2019'}t available right now, so these cards use Yard{'\u2019'}s
                  standard wording and order. The error is under Details.
                </span>
              )}
          </div>
        ) : null}
        {!empty ? (
          <div className="board-toolbar">
            {outOfDate ? (
              <button
                className="board-pill"
                disabled={running}
                onClick={() => onGenerate(false)}
                type="button"
              >
                {outOfDateLabel(board)}
              </button>
            ) : null}
            <input
              aria-label="Filter cards"
              className="board-search"
              onChange={(event) => onFilterChange({ ...filter, text: event.target.value })}
              placeholder="Search (press /)"
              ref={filterInput}
              type="search"
              value={filter.text}
            />
            <label className="board-closed-toggle">
              <input
                checked={filter.showClosed}
                onChange={(event) =>
                  onFilterChange({ ...filter, showClosed: event.target.checked })
                }
                type="checkbox"
              />
              Show closed cards ({board?.counts?.closed ?? 0})
            </label>
            <label
              className="board-closed-toggle"
              title="Subagents, Yard's own summary and temporary sessions, and agents nobody talks to"
            >
              <input
                checked={showHelpers}
                onChange={(event) => setShowHelpers(event.target.checked)}
                type="checkbox"
              />
              Show helpers ({board?.helpers ?? 0})
            </label>
          </div>
        ) : null}
        <div className="board-body">
          {empty ? (
            <div className="board-empty">
              <ClipboardList aria-hidden="true" size={28} />
              <h3>See all your open agents in one place</h3>
              <p>
                Press Refresh board to read the agents you have open in a
                terminal right now. You get one list, most urgent first: what
                needs you, what is working, and what has gone quiet. Each line
                says what to do next; click it for the details. Nothing is
                changed. Every button opens its usual dialog first.
              </p>
              <button
                className="board-button board-button--primary"
                disabled={running}
                onClick={() => onGenerate(false)}
                type="button"
              >
                Refresh board
              </button>
            </div>
          ) : (
            <div className="board-groups">
              {list.needs.length > 0 ? (
                <section aria-label="Needs you" className="board-group" data-group="needs">
                  <h3>
                    Needs you <small>({list.needs.length})</small>
                  </h3>
                  {renderRows('needs', list.needs, 'Needs you')}
                </section>
              ) : (
                <p className="board-nothing" data-testid="board-nothing">
                  Nothing needs you right now.
                </p>
              )}
              {list.working.length > 0 ? (
                <section aria-label="Working" className="board-group" data-group="working">
                  <h3>
                    Working <small>({list.working.length})</small>
                  </h3>
                  {renderRows('working', list.working, 'Working')}
                </section>
              ) : null}
              {list.quiet.length > 0 ? (
                <section aria-label="Quiet" className="board-group" data-group="quiet">
                  <h3>
                    Quiet <small>({list.quiet.length})</small>{' '}
                    <button
                      aria-expanded={quietOpen}
                      className="board-link"
                      onClick={() => setQuietOpen((open) => !open)}
                      type="button"
                    >
                      <ChevronDown aria-hidden="true" data-open={quietOpen || undefined} size={13} />
                      {quietOpen ? 'hide' : 'show'}
                    </button>
                  </h3>
                  {quietOpen ? renderRows('quiet', list.quiet, 'Quiet') : null}
                </section>
              ) : null}
              {list.closed.length > 0 ? (
                <section aria-label="Closed" className="board-group" data-group="closed">
                  <h3>
                    Closed <small>({list.closed.length})</small>
                  </h3>
                  {renderRows('closed', list.closed, 'Closed')}
                </section>
              ) : null}
              {older.length > 0 ? (
                <section aria-label="Older" className="board-group" data-group="older" data-testid="board-older">
                  <h3>
                    Older <small>({older.length})</small>{' '}
                    {toReview.length > 0 && onTriage ? (
                      <button
                        className="board-link"
                        onClick={() => {
                          setReviewStart(0)
                          setReviewing(true)
                        }}
                        title={`One card at a time, the ones suggested for archive first (${toReview.length} to review)`}
                        type="button"
                      >
                        review
                      </button>
                    ) : null}
                  </h3>
                  <p className="board-muted" data-testid="board-older-line">
                    {olderSummary
                      ? `${olderSummary} Nothing here is lost.`
                      : 'Quiet longer than the board’s window and not waiting on you. Nothing here is lost.'}
                  </p>
                </section>
              ) : null}
              {showHelpers ? (
                <section aria-label="Helpers" className="board-group" data-group="helper" data-testid="board-helpers">
                  <h3>
                    Helpers <small>({board?.helpers ?? 0})</small>
                  </h3>
                  <p className="board-muted">
                    Agents that work for other agents or for Yard. Shown only on request.
                  </p>
                  {renderRows('helper', helpers, 'Helpers')}
                  {(board?.helper_rows ?? []).length > 0 ? (
                    <ul aria-label="Subagents" className="board-helper-rows">
                      {(board?.helper_rows ?? []).map((row) => (
                        <li key={row.id}>
                          <strong>{row.title}</strong>
                          <span className="board-muted">
                            {` Subagent of ${row.parent_title ?? 'an agent not on the board'}${SEP}${row.status}${SEP}${formatAgo(row.age_ms)}`}
                          </span>
                        </li>
                      ))}
                    </ul>
                  ) : null}
                </section>
              ) : null}
            </div>
          )}
        </div>
        {reviewing && onTriage ? (
          <ReviewOlderWork
            items={toReview}
            start={reviewStart}
            onAction={onAction}
            onClose={() => {
              setReviewing(false)
              window.requestAnimationFrame(() => workspace.current?.focus())
            }}
            onTriage={onTriage}
          />
        ) : null}
        <footer className="board-footer">
          <span title={board?.warnings.length ? `Skipped: ${board.warnings.join('; ')}` : undefined}>
            {board?.warnings.length
              ? `${board.warnings.length} source${board.warnings.length === 1 ? '' : 's'} could not be read`
              : ''}
          </span>
          {board?.hidden && board.hidden.not_live > 0 ? (
            <button
              className="board-button board-button--quiet board-hidden-link"
              onClick={(event) => onCleanUp?.(event.currentTarget)}
              title={`Agents whose terminal has ended. They are hidden from the board${
                board.hidden.dead_runtimes > 0
                  ? `; ${board.hidden.dead_runtimes} can\u2019t be started again`
                  : ''
              }. Clean up hides them from Yard; their tasks, reports and history are kept.`}
              type="button"
            >
              {board.hidden.not_live} ended agent{board.hidden.not_live === 1 ? '' : 's'} not on the board
              {board.hidden.dead_runtimes > 0
                ? ` (${board.hidden.dead_runtimes} can\u2019t be resumed)`
                : ''}
              {SEP}Clean up{'\u2026'}
            </button>
          ) : null}
          <button
            aria-expanded={helpOpen}
            className="board-button board-button--quiet"
            onClick={() => setHelpOpen((open) => !open)}
            type="button"
          >
            Keyboard shortcuts
          </button>
        </footer>
        {helpOpen ? (
          <div aria-label="Keyboard shortcuts" className="board-help" role="region">
            <dl>
              {KEYBOARD_HELP.map(([keys, label]) => (
                <div key={keys}>
                  <dt>
                    <kbd>{keys}</kbd>
                  </dt>
                  <dd>{label}</dd>
                </div>
              ))}
            </dl>
          </div>
        ) : null}
      </section>
    </div>
  )
}

export function WorkBoardRailPanel({
  board,
  onOpen,
}: {
  board: WorkBoard | null
  /** Open the board; `generate` also starts a generation. */
  onOpen: (trigger: HTMLElement, generate: boolean) => void
}) {
  const empty = !board || board.state === 'empty'
  const counts = board?.counts
  // The top of Needs you, the same lines as the board.
  const needs = boardList(board?.items ?? [], EMPTY_FILTER).needs.slice(0, 5)
  const tiles = [
    { id: 'doing', label: BOARD_COLUMNS[0]?.label, value: counts?.doing ?? 0 },
    { id: 'needs_you', label: BOARD_COLUMNS[1]?.label, value: counts?.needs_you ?? 0 },
    {
      id: 'stale',
      label: BOARD_COLUMNS[2]?.label,
      value: (counts?.stale_needs_work ?? 0) + (counts?.stale_likely_done ?? 0),
    },
    { id: 'close', label: BOARD_COLUMNS[3]?.label, value: counts?.close ?? 0 },
  ]
  const button = (
    <button
      className="board-button board-button--primary"
      onClick={(event) => onOpen(event.currentTarget, empty)}
      type="button"
    >
      {empty ? 'Refresh board' : 'Open board'}
    </button>
  )
  return (
    <div className="rail-section rail-section--resources board-rail" role="tabpanel">
      <div className="section-heading">
        <p className="eyebrow">Across projects</p>
        <h2>Work board</h2>
      </div>
      <div className="board-rail__body">
        {empty ? (
          <div className="board-rail__top">
            <p className="board-muted">
              {board?.state === 'running'
                ? 'Refreshing\u2026'
                : 'One view of your open agents: what is working, what needs you, and what you can close.'}
            </p>
            {button}
          </div>
        ) : (
          <>
            <div className="board-rail__top">
              <ul aria-label="Board counts" className="board-rail__tiles">
                {tiles.map((tile) => (
                  <li data-column={tile.id} key={tile.id}>
                    <strong>{tile.value}</strong>
                    <span>{tile.label}</span>
                  </li>
                ))}
              </ul>
              {button}
            </div>
            {needs.length > 0 ? (
              <ol aria-label="Needs you" className="board-rail__todos">
                {needs.map((item) => (
                  <li key={item.id}>
                    <strong>{item.title}</strong>
                    <span>{` \u2014 ${rowLine(item, 'needs')}`}</span>
                  </li>
                ))}
              </ol>
            ) : (
              <p className="board-muted">Nothing needs you right now.</p>
            )}
          </>
        )}
      </div>
    </div>
  )
}

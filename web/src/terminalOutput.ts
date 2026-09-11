import type { ObservedStatus } from './types'

export const TERMINAL_OUTPUT_LINES = 1_000
export const TERMINAL_HISTORY_LINES = 10_000
export const TRUNCATED_TERMINAL_OUTPUT_LABEL =
  'latest 1,000 lines · earlier output unavailable'

export function normalizeTerminalOutput(text: string) {
  return trimBlankLines(
    text.replace(/\r\n?/g, '\n').split('\n'),
  ).join('\n')
}

export interface TerminalTranscriptTurn {
  answer: string
  question: string
  work: string
}

export interface TerminalTranscript {
  latestQuestion: string | null
  preamble: string
  structured: boolean
  turns: TerminalTranscriptTurn[]
}

const PROMPT_LINE = /^[›❯]\s?/
const CODEX_PROMPT_LINE = /^›\s?/
const CODEX_ASSISTANT_LINE = /^•\s?/
const CLAUDE_ASSISTANT_LINE = /^⏺\s?/
const ASSISTANT_LINE = /^[•⏺]\s?/
const DIVIDER_LINE = /^\s*[─━═-]{8,}\s*$/
const MANUAL_CONTRACT_LINE = /^Exit contract:\s*$/
const INVALID_COMMAND_LINE = /^invalid_command\s*$/
const TERMINAL_CHROME_LINE =
  /^(?:Theme|Goal (?:active|paused|complete)|.*·\s*~\/.*(?:Goal|context).*)$/
const CLEAR_TOOL_ACTIVITY_HEADING =
  /^(?:(?:Explored|Ran|Read|Searched|Listed|Inspected|Checked|Called|Waited)$|(?:Bash|Glob|Grep|Search|Read|Edit|Write|Update)\()/
const AMBIGUOUS_TOOL_ACTIVITY_HEADING =
  /^(?:Ran|Read|Searched|Listed|Inspected|Checked|Called|Waited)\b/
const TOOL_ACTIVITY_DETAIL = /^\s*[└├│]/
const CLEAR_PROGRESS_HEADINGS = [
  /^I(?:'|’)m\s+(?:checking|inspecting|reviewing|running|reading|searching|analyzing|implementing|testing|building|waiting|working)\b/,
  /^(?:Now|Checking|Inspecting|Reviewing|Running|Reading|Searching|Analyzing|Implementing|Testing|Building|Waiting)\b/,
]
const ACTIVE_PROGRESS_HEADING =
  /^(?:Working\b|Continuing\b|Still\b)/

function trimBlankLines(lines: string[]) {
  let start = 0
  let end = lines.length
  while (start < end && lines[start].trim() === '') start += 1
  while (end > start && lines[end - 1].trim() === '') end -= 1
  return lines.slice(start, end)
}

function cleanTranscriptLines(lines: string[]) {
  return trimBlankLines(
    lines.filter(
      (line) =>
        !DIVIDER_LINE.test(line) &&
        !TERMINAL_CHROME_LINE.test(line.trim()),
    ),
  )
}

function cleanQuestion(lines: string[]) {
  const cleaned = cleanTranscriptLines(lines)
  if (cleaned.length === 0) return ''
  cleaned[0] = cleaned[0].replace(PROMPT_LINE, '')
  const contractIndex = cleaned.findIndex((line) =>
    MANUAL_CONTRACT_LINE.test(line.trim()),
  )
  const invalidCommandIndex = cleaned.findIndex((line) =>
    INVALID_COMMAND_LINE.test(line.trim()),
  )
  const end = [contractIndex, invalidCommandIndex]
    .filter((index) => index >= 0)
    .reduce((earliest, index) => Math.min(earliest, index), cleaned.length)
  return trimBlankLines(cleaned.slice(0, end)).join('\n')
}

function assistantBlocks(lines: string[]) {
  const starts = lines.flatMap((line, index) =>
    ASSISTANT_LINE.test(line) ? [index] : [],
  )
  return starts.map((start, index) =>
    cleanTranscriptLines(
      lines.slice(start, starts[index + 1] ?? lines.length),
    ),
  )
}

function blockHeading(block: string[]) {
  return block[0]?.replace(ASSISTANT_LINE, '').trim() ?? ''
}

function blockText(block: string[], keepMarker: boolean) {
  if (block.length === 0) return ''
  const lines = [...block]
  if (!keepMarker) lines[0] = lines[0].replace(ASSISTANT_LINE, '')
  return trimBlankLines(lines).join('\n')
}

function isWorkOnlyBlock(block: string[], status: ObservedStatus) {
  const heading = blockHeading(block)
  if (CLEAR_TOOL_ACTIVITY_HEADING.test(heading)) return true
  if (
    AMBIGUOUS_TOOL_ACTIVITY_HEADING.test(heading) &&
    (status === 'working' ||
      status === 'unknown' ||
      block.slice(1).some((line) => TOOL_ACTIVITY_DETAIL.test(line)))
  ) {
    return true
  }
  if (
    CLEAR_PROGRESS_HEADINGS.some((pattern) => pattern.test(heading))
  ) {
    return true
  }
  return (
    (status === 'working' || status === 'unknown') &&
    ACTIVE_PROGRESS_HEADING.test(heading)
  )
}

function parseTurn(
  lines: string[],
  status: ObservedStatus,
): TerminalTranscriptTurn {
  const firstAssistant = lines.findIndex((line) =>
    ASSISTANT_LINE.test(line),
  )
  const questionLines =
    firstAssistant >= 0 ? lines.slice(0, firstAssistant) : lines
  const blocks =
    firstAssistant >= 0 ? assistantBlocks(lines.slice(firstAssistant)) : []
  const lastBlock = blocks.at(-1)
  const hasAnswer =
    Boolean(lastBlock) && !isWorkOnlyBlock(lastBlock ?? [], status)
  const workBlocks = hasAnswer ? blocks.slice(0, -1) : blocks
  return {
    answer: hasAnswer ? blockText(lastBlock ?? [], false) : '',
    question: cleanQuestion(questionLines),
    work: workBlocks
      .map((block) => blockText(block, true))
      .filter(Boolean)
      .join('\n\n'),
  }
}

function parseTruncatedTurn(
  lines: string[],
  status: ObservedStatus,
): TerminalTranscriptTurn {
  const firstAssistant = lines.findIndex((line) =>
    ASSISTANT_LINE.test(line),
  )
  if (firstAssistant < 0) {
    return {
      answer: '',
      question: '',
      work: cleanTranscriptLines(lines).join('\n'),
    }
  }

  const leadingWork = cleanTranscriptLines(
    lines.slice(0, firstAssistant),
  ).join('\n')
  const turn = parseTurn(
    ['›', ...lines.slice(firstAssistant)],
    status,
  )
  return {
    ...turn,
    work: [leadingWork, turn.work].filter(Boolean).join('\n\n'),
  }
}

export function parseTerminalTranscript(
  text: string,
  status: ObservedStatus = 'unknown',
  truncated = false,
): TerminalTranscript {
  const normalized = normalizeTerminalOutput(text)
  if (!normalized) {
    return {
      latestQuestion: null,
      preamble: '',
      structured: false,
      turns: [],
    }
  }

  const lines = normalized.split('\n')
  const promptCandidates = lines.flatMap((line, index) =>
    PROMPT_LINE.test(line) ? [index] : [],
  )
  const promptStarts = promptCandidates.filter((start, index) =>
    lines
      .slice(start + 1, promptCandidates[index + 1] ?? lines.length)
      .some((line) =>
        (CODEX_PROMPT_LINE.test(lines[start])
          ? CODEX_ASSISTANT_LINE
          : CLAUDE_ASSISTANT_LINE
        ).test(line),
      ),
  )
  if (promptStarts.length !== promptCandidates.length) {
    return {
      latestQuestion: null,
      preamble: normalized,
      structured: false,
      turns: [],
    }
  }
  if (promptStarts.length === 0) {
    if (truncated && lines.some((line) => ASSISTANT_LINE.test(line))) {
      return {
        latestQuestion: null,
        preamble: '',
        structured: true,
        turns: [parseTruncatedTurn(lines, status)],
      }
    }
    return {
      latestQuestion: null,
      preamble: normalized,
      structured: false,
      turns: [],
    }
  }

  const turns = promptStarts.map((start, index) =>
    parseTurn(
      lines.slice(start, promptStarts[index + 1] ?? lines.length),
      index === promptStarts.length - 1 ? status : 'done',
    ),
  )
  let preamble = cleanTranscriptLines(
    lines.slice(0, promptStarts[0]),
  ).join('\n')
  if (truncated && preamble) {
    turns[0] = {
      ...turns[0],
      work: [preamble, turns[0].work].filter(Boolean).join('\n\n'),
    }
    preamble = ''
  }
  return {
    latestQuestion:
      [...turns]
        .reverse()
        .find((turn) => turn.question)?.question ?? null,
    preamble,
    structured: true,
    turns,
  }
}

import { describe, expect, it } from 'vitest'
import { parseTerminalTranscript } from './terminalOutput'

describe('parseTerminalTranscript', () => {
  it('separates Codex progress and tool activity from the final answer', () => {
    const transcript = parseTerminalTranscript(
      [
        'Codex session header',
        '› Fix the upload path.',
        '',
        '• I’ll inspect the route first.',
        '',
        '• Explored',
        '  └ Read upload.ts',
        '',
        '• Updated the route and added a regression test.',
      ].join('\n'),
      'done',
    )

    expect(transcript).toEqual({
      latestQuestion: 'Fix the upload path.',
      preamble: 'Codex session header',
      structured: true,
      turns: [
        {
          answer: 'Updated the route and added a regression test.',
          question: 'Fix the upload path.',
          work: [
            '• I’ll inspect the route first.',
            '• Explored\n  └ Read upload.ts',
          ].join('\n\n'),
        },
      ],
    })
  })

  it('keeps a working turn tool-only when no answer exists yet', () => {
    const transcript = parseTerminalTranscript(
      [
        '› Check the failing tests.',
        '',
        '• Explored',
        '  └ Read test.log',
        '',
        '• I’m running the focused suite now.',
      ].join('\n'),
      'working',
    )

    expect(transcript.turns[0]).toEqual({
      answer: '',
      question: 'Check the failing tests.',
      work: [
        '• Explored\n  └ Read test.log',
        '• I’m running the focused suite now.',
      ].join('\n\n'),
    })
  })

  it('keeps clear progress collapsed after durable status catches up', () => {
    const transcript = parseTerminalTranscript(
      [
        '› Verify the production build.',
        '• I’m running the production build now.',
      ].join('\n'),
      'done',
    )

    expect(transcript.turns[0]).toEqual({
      answer: '',
      question: 'Verify the production build.',
      work: '• I’m running the production build now.',
    })
  })

  it('does not mistake a completion statement for progress', () => {
    const transcript = parseTerminalTranscript(
      [
        '› Verify the production build.',
        '• I’m done. The production build passes.',
      ].join('\n'),
      'done',
    )

    expect(transcript.turns[0]).toEqual({
      answer: 'I’m done. The production build passes.',
      question: 'Verify the production build.',
      work: '',
    })
  })

  it('keeps completed answers beginning with Next or I’ll visible', () => {
    const next = parseTerminalTranscript(
      [
        '› What should I do after the build?',
        '• Next, deploy the tested artifact.',
      ].join('\n'),
      'done',
    )
    const future = parseTerminalTranscript(
      [
        '› Can you own the follow-up?',
        '• I’ll prepare the rollout checklist tomorrow.',
      ].join('\n'),
      'done',
    )

    expect(next.turns[0]?.answer).toBe(
      'Next, deploy the tested artifact.',
    )
    expect(future.turns[0]?.answer).toBe(
      'I’ll prepare the rollout checklist tomorrow.',
    )
  })

  it('does not mistake an answer beginning with Ready for a Read tool', () => {
    const transcript = parseTerminalTranscript(
      [
        '› Is this ready for review?',
        '• Ready for owner review.',
      ].join('\n'),
      'done',
    )

    expect(transcript.turns[0]).toEqual({
      answer: 'Ready for owner review.',
      question: 'Is this ready for review?',
      work: '',
    })
  })

  it('keeps a completed Ran summary visible as an answer', () => {
    const transcript = parseTerminalTranscript(
      [
        '› Did the focused tests pass?',
        '• Ran the focused tests; all 12 pass.',
      ].join('\n'),
      'done',
    )

    expect(transcript.turns[0]).toEqual({
      answer: 'Ran the focused tests; all 12 pass.',
      question: 'Did the focused tests pass?',
      work: '',
    })
  })

  it('recognizes a Ran block with terminal detail as tool activity', () => {
    const transcript = parseTerminalTranscript(
      [
        '› Run the focused tests.',
        '• Ran npm test',
        '  └ 12 tests passed',
      ].join('\n'),
      'done',
    )

    expect(transcript.turns[0]).toEqual({
      answer: '',
      question: 'Run the focused tests.',
      work: '• Ran npm test\n  └ 12 tests passed',
    })
  })

  it('parses multiple turns and returns the latest question', () => {
    const transcript = parseTerminalTranscript(
      [
        '› What failed?',
        '• The upload test failed.',
        '› Can you fix it?',
        '• Read(upload.ts)',
        '• The upload path now handles spaces.',
      ].join('\n'),
      'done',
    )

    expect(transcript.latestQuestion).toBe('Can you fix it?')
    expect(transcript.turns).toEqual([
      {
        answer: 'The upload test failed.',
        question: 'What failed?',
        work: '',
      },
      {
        answer: 'The upload path now handles spaces.',
        question: 'Can you fix it?',
        work: '• Read(upload.ts)',
      },
    ])
  })

  it('removes Yard intervention metadata from the displayed question', () => {
    const transcript = parseTerminalTranscript(
      [
        '› Re-run the focused test.',
        '',
        'Exit contract:',
        '- Report progress and blockers.',
        'invalid_command',
        'Theme',
        '',
        '• The focused test passes.',
      ].join('\n'),
      'done',
    )

    expect(transcript.latestQuestion).toBe('Re-run the focused test.')
    expect(transcript.turns[0]?.answer).toBe(
      'The focused test passes.',
    )
  })

  it('leaves unstructured output fully visible', () => {
    const transcript = parseTerminalTranscript(
      'build started\nstep one complete\nbuild finished',
      'done',
    )

    expect(transcript).toEqual({
      latestQuestion: null,
      preamble: 'build started\nstep one complete\nbuild finished',
      structured: false,
      turns: [],
    })
  })

  it('does not mistake an ordinary Claude-style shell prompt for a question', () => {
    const text = [
      '❯ printf "building\\n"',
      'building',
      'build finished',
    ].join('\n')
    const transcript = parseTerminalTranscript(text, 'done')

    expect(transcript).toEqual({
      latestQuestion: null,
      preamble: text,
      structured: false,
      turns: [],
    })
  })

  it('does not mistake shell output beginning with a bullet for a Claude turn', () => {
    const text = [
      '❯ cat release-notes.txt',
      '• fixed terminal history',
      '• preserved shell output',
    ].join('\n')
    const transcript = parseTerminalTranscript(text, 'done')

    expect(transcript).toEqual({
      latestQuestion: null,
      preamble: text,
      structured: false,
      turns: [],
    })
  })

  it('falls back to raw output when shell activity follows an agent turn', () => {
    const text = [
      '› Run the build.',
      '• The build passes.',
      '❯ git status --short',
      ' M src/build.ts',
    ].join('\n')
    const transcript = parseTerminalTranscript(text, 'done')

    expect(transcript).toEqual({
      latestQuestion: null,
      preamble: text,
      structured: false,
      turns: [],
    })
  })

  it('collapses a truncated tail after its prompt was cut off', () => {
    const transcript = parseTerminalTranscript(
      [
        '  • tool output retained from the cutoff',
        '  └ final check passed',
        '',
        '• The build is ready for review.',
      ].join('\n'),
      'done',
      true,
    )

    expect(transcript).toEqual({
      latestQuestion: null,
      preamble: '',
      structured: true,
      turns: [
        {
          answer: 'The build is ready for review.',
          question: '',
          work: [
            '  • tool output retained from the cutoff',
            '  └ final check passed',
          ].join('\n'),
        },
      ],
    })
  })

  it('keeps a markerless truncated tail visible when it cannot classify it', () => {
    const text = 'build step 998\nbuild step 999\nbuild step 1000'
    const transcript = parseTerminalTranscript(
      text,
      'working',
      true,
    )

    expect(transcript).toEqual({
      latestQuestion: null,
      preamble: text,
      structured: false,
      turns: [],
    })
  })

  it('folds a truncated leading fragment into the next turn work', () => {
    const transcript = parseTerminalTranscript(
      [
        'orphaned output from the previous turn',
        '› What is the current result?',
        '• The focused checks pass.',
      ].join('\n'),
      'done',
      true,
    )

    expect(transcript.preamble).toBe('')
    expect(transcript.turns[0]).toEqual({
      answer: 'The focused checks pass.',
      question: 'What is the current result?',
      work: 'orphaned output from the previous turn',
    })
  })

  it('keeps indented assistant glyphs inside the final answer', () => {
    const transcript = parseTerminalTranscript(
      [
        '› Summarize the findings.',
        '• Findings:',
        '  • First issue',
        '  ⏺ Second issue',
      ].join('\n'),
      'done',
    )

    expect(transcript.turns[0]).toEqual({
      answer: 'Findings:\n  • First issue\n  ⏺ Second issue',
      question: 'Summarize the findings.',
      work: '',
    })
  })

  it('recognizes Claude prompt and assistant markers', () => {
    const transcript = parseTerminalTranscript(
      [
        '❯ Audit the retry logic.',
        '',
        '⏺ Read(retry.ts)',
        '',
        '⏺ Retries are bounded and preserve the command ID.',
      ].join('\n'),
      'done',
    )

    expect(transcript.turns[0]).toEqual({
      answer: 'Retries are bounded and preserve the command ID.',
      question: 'Audit the retry logic.',
      work: '⏺ Read(retry.ts)',
    })
  })
})

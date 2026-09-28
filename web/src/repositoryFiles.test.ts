import { describe, expect, it } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import { createElement } from 'react'
import { DiffView } from './RepositoryFilesWorkspace'
import {
  buildRepositoryReviewPrompt,
  buildRepositoryTree,
  fuzzyRepositoryFiles,
  moveRepositorySelection,
  repositoryDiffRows,
  visibleRepositoryFiles,
} from './repositoryFiles'
import type { RepositoryDiff, RepositoryFile } from './types'

const file = (path: string): RepositoryFile => ({
  additions: null,
  deletions: null,
  path,
  previous_path: null,
  staged: false,
  state: 'tracked',
  unstaged: false,
})

describe('repository file navigation', () => {
  it('builds a sorted collapsible tree', () => {
    const tree = buildRepositoryTree([
      file('z.txt'),
      file('src/b.ts'),
      file('src/a.ts'),
    ])
    expect(tree.map((node) => node.name)).toEqual(['src', 'z.txt'])
    expect(tree[0]?.children.map((node) => node.name)).toEqual([
      'a.ts',
      'b.ts',
    ])
    expect(visibleRepositoryFiles(tree, new Set())).toEqual([file('z.txt')])
    expect(
      visibleRepositoryFiles(tree, new Set(['src'])).map(
        (candidate) => candidate.path,
      ),
    ).toEqual(['src/a.ts', 'src/b.ts', 'z.txt'])
  })

  it('supports fuzzy filtering and bounded j/k movement', () => {
    const files = [file('src/alpha.ts'), file('tests/alpha.test.ts')]
    expect(fuzzyRepositoryFiles(files, 'srca').map(({ path }) => path)).toEqual([
      'src/alpha.ts',
    ])
    expect(moveRepositorySelection(-1, 2, 1)).toBe(0)
    expect(moveRepositorySelection(0, 2, -1)).toBe(0)
    expect(moveRepositorySelection(1, 2, 1)).toBe(1)
  })
})

describe('repository review comments', () => {
  it('batches structured anchors into one prompt', () => {
    const prompt = buildRepositoryReviewPrompt([
      {
        body: 'Handle the empty case.',
        endLine: 12,
        path: 'src/a.ts',
        repositoryId: 'repo-1',
        rootPath: '/repo',
        snippet: 'return value',
        startLine: 12,
      },
      {
        body: 'Keep this bounded.',
        endLine: 24,
        path: 'src/b.ts',
        repositoryId: 'repo-1',
        rootPath: '/repo',
        snippet: 'for item in items',
        startLine: 20,
      },
    ])
    expect(prompt).toContain('1. src/a.ts (line 12)')
    expect(prompt).toContain('2. src/b.ts (lines 20-24)')
    expect(prompt.match(/Repository review feedback/g)).toHaveLength(1)
  })

  it('renders 2,000 structured lines under the render budget', () => {
    const diff: RepositoryDiff = {
      binary: false,
      hunks: [
        {
          lines: Array.from({ length: 2_000 }, (_, index) => ({
            content: `line ${index}`,
            kind: 'context' as const,
            new_line: index + 1,
            old_line: index + 1,
          })),
          new_lines: 2_000,
          new_start: 1,
          old_lines: 2_000,
          old_start: 1,
        },
      ],
      path: 'large.txt',
      previous_path: null,
      repository_id: 'repo-1',
      root_path: '/repo',
      truncated: false,
      unavailable: false,
    }
    const started = performance.now()
    expect(repositoryDiffRows(diff)).toHaveLength(2_000)
    const markup = renderToStaticMarkup(
      createElement(DiffView, {
        diff,
        hunkRefs: { current: [] },
        onComment: () => undefined,
        onSelectHunk: () => undefined,
        selectedHunk: 0,
      }),
    )
    expect(markup).toContain('line 1999')
    expect(performance.now() - started).toBeLessThan(200)
  })
})

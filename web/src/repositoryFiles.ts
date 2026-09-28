import type {
  RepositoryDiff,
  RepositoryDiffLine,
  RepositoryFile,
} from './types'

export interface RepositoryTreeNode {
  children: RepositoryTreeNode[]
  file: RepositoryFile | null
  name: string
  path: string
  type: 'directory' | 'file'
}

export interface RepositoryReviewComment {
  body: string
  endLine: number | null
  path: string
  repositoryId: string
  rootPath: string
  snippet: string
  startLine: number | null
}

export interface RepositoryDiffRow {
  hunkIndex: number
  line: RepositoryDiffLine
  lineIndex: number
}

export function buildRepositoryTree(
  files: RepositoryFile[],
): RepositoryTreeNode[] {
  const roots: RepositoryTreeNode[] = []
  for (const file of files) {
    const parts = file.path.split('/').filter(Boolean)
    let children = roots
    let currentPath = ''
    parts.forEach((part, index) => {
      currentPath = currentPath ? `${currentPath}/${part}` : part
      const type = index === parts.length - 1 ? 'file' : 'directory'
      let node = children.find(
        (candidate) =>
          candidate.name === part && candidate.type === type,
      )
      if (!node) {
        node = {
          children: [],
          file: type === 'file' ? file : null,
          name: part,
          path: currentPath,
          type,
        }
        children.push(node)
      }
      children = node.children
    })
  }
  const sort = (nodes: RepositoryTreeNode[]) => {
    nodes.sort(
      (left, right) =>
        Number(left.type === 'file') - Number(right.type === 'file') ||
        left.name.localeCompare(right.name),
    )
    nodes.forEach((node) => sort(node.children))
  }
  sort(roots)
  return roots
}

export function visibleRepositoryFiles(
  tree: RepositoryTreeNode[],
  expanded: ReadonlySet<string>,
): RepositoryFile[] {
  const files: RepositoryFile[] = []
  const visit = (nodes: RepositoryTreeNode[]) => {
    for (const node of nodes) {
      if (node.file) files.push(node.file)
      if (node.type === 'directory' && expanded.has(node.path)) {
        visit(node.children)
      }
    }
  }
  visit(tree)
  return files
}

export function fuzzyRepositoryFiles(
  files: RepositoryFile[],
  query: string,
): RepositoryFile[] {
  const needle = query.trim().toLocaleLowerCase()
  if (!needle) return files
  return files.filter((file) => isSubsequence(needle, file.path.toLocaleLowerCase()))
}

function isSubsequence(needle: string, value: string) {
  let index = 0
  for (const character of value) {
    if (character === needle[index]) index += 1
    if (index === needle.length) return true
  }
  return false
}

export function moveRepositorySelection(
  current: number,
  length: number,
  direction: -1 | 1,
) {
  if (length === 0) return -1
  if (current < 0) return direction === 1 ? 0 : length - 1
  return Math.max(0, Math.min(length - 1, current + direction))
}

export function repositoryDiffRows(
  diff: RepositoryDiff,
): RepositoryDiffRow[] {
  return diff.hunks.flatMap((hunk, hunkIndex) =>
    hunk.lines.map((line, lineIndex) => ({
      hunkIndex,
      line,
      lineIndex,
    })),
  )
}

export function buildRepositoryReviewPrompt(
  comments: RepositoryReviewComment[],
) {
  const sections = comments.map((comment, index) => {
    const line =
      comment.startLine === null
        ? 'hunk'
        : comment.endLine && comment.endLine !== comment.startLine
          ? `lines ${comment.startLine}-${comment.endLine}`
          : `line ${comment.startLine}`
    return [
      `${index + 1}. ${comment.path} (${line})`,
      `Repository: ${comment.rootPath} [${comment.repositoryId}]`,
      'Snippet:',
      indent(comment.snippet),
      'Comment:',
      indent(comment.body),
    ].join('\n')
  })
  return [
    'Repository review feedback',
    '',
    ...sections.flatMap((section, index) =>
      index === sections.length - 1 ? [section] : [section, ''],
    ),
  ].join('\n')
}

function indent(value: string) {
  return value
    .split('\n')
    .map((line) => `    ${line}`)
    .join('\n')
}

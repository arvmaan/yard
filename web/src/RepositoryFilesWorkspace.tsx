import {
  ChevronDown,
  ChevronRight,
  File,
  FileDiff,
  Folder,
  GitBranch,
  Link,
  MessageSquareText,
  Search,
  Send,
  Trash2,
  X,
} from 'lucide-react'
import {
  useEffect,
  useCallback,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  type MutableRefObject,
} from 'react'
import {
  fetchProjectRepositories,
  fetchRepositoryDiff,
  fetchRepositoryFileContent,
  fetchRepositoryFiles,
  linkProjectRepository,
  relinkProjectRepository,
  sendAssignmentPrompt,
  unlinkProjectRepository,
} from './api'
import type { AgentWorkspaceTarget } from './AgentWorkspaceContext'
import {
  buildRepositoryReviewPrompt,
  buildRepositoryTree,
  fuzzyRepositoryFiles,
  moveRepositorySelection,
  visibleRepositoryFiles,
  type RepositoryReviewComment,
  type RepositoryTreeNode,
} from './repositoryFiles'
import type {
  Assignment,
  Project,
  ProjectRepositories,
  ProjectRepository,
  PromptAcknowledgement,
  RepositoryDiff,
  RepositoryDiffLine,
  RepositoryFile,
  RepositoryFileContent,
  RepositoryFileMode,
} from './types'

function errorMessage(error: unknown, fallback: string) {
  return error instanceof Error ? error.message : fallback
}

export function ProjectRepositoriesSection({
  onChanged,
  project,
  suggestedRoot,
}: {
  onChanged: () => void
  project: Project
  suggestedRoot?: string
}) {
  const [repositories, setRepositories] = useState<ProjectRepository[]>([])
  const [rootPath, setRootPath] = useState('')
  const [editingId, setEditingId] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(
    async (signal?: AbortSignal) => {
      const result = await fetchProjectRepositories(project.id, signal)
      setRepositories(result.repositories)
    },
    [project.id],
  )

  useEffect(() => {
    const controller = new AbortController()
    setError(null)
    void load(controller.signal).catch((caught) => {
      if (!controller.signal.aborted) {
        setError(errorMessage(caught, 'Repositories could not be loaded'))
      }
    })
    return () => controller.abort()
  }, [load])

  const submit = async (event: FormEvent) => {
    event.preventDefault()
    const path = rootPath.trim()
    if (busy) return
    if (!path) {
      setError('Enter an absolute Git checkout path before linking a repository.')
      return
    }
    if (!path.startsWith('/')) {
      setError('Repository checkout path must be absolute.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      if (editingId) {
        await relinkProjectRepository(project.id, editingId, {
          root_path: path,
        })
      } else {
        await linkProjectRepository(project.id, { root_path: path })
      }
      setEditingId(null)
      setRootPath('')
      await load()
      onChanged()
    } catch (caught) {
      setError(errorMessage(caught, 'Repository could not be linked'))
    } finally {
      setBusy(false)
    }
  }

  return (
    <section
      aria-label="Project repositories"
      className="project-repositories"
      id={`project-repositories-${project.id}`}
      tabIndex={-1}
    >
      <div className="project-repositories__heading">
        <span>
          <p className="eyebrow">Repositories</p>
          <small>Durable project checkout roots</small>
        </span>
        <GitBranch aria-hidden="true" size={15} />
      </div>
      {repositories.map((repository) => (
        <article
          className="project-repository"
          data-repository-id={repository.id}
          key={repository.id}
        >
          <dl>
            <div>
              <dt>Canonical root</dt>
              <dd><code>{repository.root_path}</code></dd>
            </div>
            <div>
              <dt>Git common dir</dt>
              <dd><code>{repository.git_common_dir}</code></dd>
            </div>
          </dl>
          <div className="project-repository__actions">
            <button
              className="secondary-button"
              onClick={() => {
                setEditingId(repository.id)
                setRootPath(repository.root_path)
                setError(null)
              }}
              type="button"
            >
              <Link aria-hidden="true" size={13} />
              Relink
            </button>
            <button
              className="destructive-button"
              onClick={() => {
                if (
                  !window.confirm(
                    `Unlink ${repository.root_path} from ${project.name}?`,
                  )
                ) {
                  return
                }
                setBusy(true)
                setError(null)
                void unlinkProjectRepository(project.id, repository.id)
                  .then(() => load())
                  .then(onChanged)
                  .catch((caught) =>
                    setError(
                      errorMessage(caught, 'Repository could not be unlinked'),
                    ),
                  )
                  .finally(() => setBusy(false))
              }}
              type="button"
            >
              <Trash2 aria-hidden="true" size={13} />
              Unlink
            </button>
          </div>
        </article>
      ))}
      <form className="project-repository-form" onSubmit={submit}>
        {suggestedRoot && !editingId && rootPath.trim() !== suggestedRoot ? (
          <button
            className="secondary-button project-repository-form__suggestion"
            onClick={() => {
              setRootPath(suggestedRoot)
              setError(null)
            }}
            type="button"
          >
            Use project checkout
            <code>{suggestedRoot}</code>
          </button>
        ) : null}
        <label>
          <span>{editingId ? 'New checkout path' : 'Absolute checkout path'}</span>
          <input
            disabled={busy}
            onChange={(event) => setRootPath(event.target.value)}
            placeholder="/absolute/path/to/repository"
            required
            type="text"
            value={rootPath}
          />
        </label>
        <div>
          <button
            className="command-button"
            disabled={busy || !rootPath.trim().startsWith('/')}
            title={
              rootPath.trim().startsWith('/')
                ? undefined
                : 'Enter an absolute Git checkout path first'
            }
            type="submit"
          >
            <Link aria-hidden="true" size={14} />
            {editingId ? 'Save relink' : 'Link repository'}
          </button>
          {editingId ? (
            <button
              className="secondary-button"
              onClick={() => {
                setEditingId(null)
                setRootPath('')
              }}
              type="button"
            >
              Cancel
            </button>
          ) : null}
        </div>
      </form>
      {error ? <p className="dialog-error" role="alert">{error}</p> : null}
    </section>
  )
}

function targetProjectId(target: AgentWorkspaceTarget | null) {
  if (target?.target.kind === 'assignment') {
    return target.target.assignment.project_id
  }
  if (target?.target.kind === 'orchestrator') {
    return target.target.project.id
  }
  if (
    target?.target.kind === 'coordination-node' &&
    target.target.node.attached_project_ids.length === 1
  ) {
    return target.target.node.attached_project_ids[0] ?? ''
  }
  return ''
}

function activeAssignmentId(target: AgentWorkspaceTarget | null) {
  return target?.target.kind === 'assignment'
    ? target.target.assignment.id
    : ''
}

export function RepositoryFilesWorkspace({
  activeTarget,
  assignments,
  label,
  onOpenProjectRepositories,
  projects,
}: {
  activeTarget: AgentWorkspaceTarget | null
  assignments: Assignment[]
  label: string
  onOpenProjectRepositories: (projectId: string) => void
  projects: Project[]
}) {
  const suggestedProjectId = targetProjectId(activeTarget)
  const [repositories, setRepositories] = useState<
    Record<string, ProjectRepositories>
  >({})
  const [selectedProjectId, setSelectedProjectId] =
    useState(suggestedProjectId)
  const [selectedRepositoryId, setSelectedRepositoryId] = useState('')
  const [mode, setMode] = useState<RepositoryFileMode>('browse')
  const [files, setFiles] = useState<RepositoryFile[]>([])
  const [filesTruncated, setFilesTruncated] = useState(false)
  const [selectedPath, setSelectedPath] = useState('')
  const [content, setContent] = useState<RepositoryFileContent | null>(null)
  const [diff, setDiff] = useState<RepositoryDiff | null>(null)
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const [query, setQuery] = useState('')
  const [filterOpen, setFilterOpen] = useState(false)
  const [selectedIndex, setSelectedIndex] = useState(-1)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [selectedHunk, setSelectedHunk] = useState(0)
  const [commentAnchor, setCommentAnchor] =
    useState<Omit<RepositoryReviewComment, 'body'> | null>(null)
  const [commentBody, setCommentBody] = useState('')
  const [comments, setComments] = useState<RepositoryReviewComment[]>([])
  const [assignmentId, setAssignmentId] =
    useState(activeAssignmentId(activeTarget))
  const [sendBusy, setSendBusy] = useState(false)
  const [sendError, setSendError] = useState<string | null>(null)
  const [receipt, setReceipt] = useState<PromptAcknowledgement | null>(null)
  const retainedCommand = useRef<{
    assignmentId: string
    commandId: string
    text: string
  } | null>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const filterRef = useRef<HTMLInputElement>(null)
  const hunkRefs = useRef<Array<HTMLElement | null>>([])
  const selectedRead = useRef<AbortController | null>(null)

  useEffect(() => {
    const controller = new AbortController()
    setLoading(true)
    setError(null)
    void Promise.all(
      projects.map(async (project) => [
        project.id,
        await fetchProjectRepositories(project.id, controller.signal),
      ] as const),
    )
      .then((entries) => {
        if (controller.signal.aborted) return
        const next = Object.fromEntries(entries)
        setRepositories(next)
        setSelectedProjectId((current) => {
          if (current && projects.some((project) => project.id === current)) {
            return current
          }
          return (
            projects.find(
              (project) => next[project.id]?.repositories.length,
            )?.id ??
            projects[0]?.id ??
            ''
          )
        })
      })
      .catch((caught) => {
        if (!controller.signal.aborted) {
          setError(errorMessage(caught, 'Repositories could not be loaded'))
        }
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [projects])

  const projectRepositories = useMemo(
    () => repositories[selectedProjectId]?.repositories ?? [],
    [repositories, selectedProjectId],
  )
  const selectedRepository = useMemo(
    () =>
      projectRepositories.find(
        (repository) => repository.id === selectedRepositoryId,
      ) ??
      projectRepositories[0] ??
      null,
    [projectRepositories, selectedRepositoryId],
  )

  useEffect(() => {
    setSelectedRepositoryId((current) =>
      projectRepositories.some((repository) => repository.id === current)
        ? current
        : (projectRepositories[0]?.id ?? ''),
    )
  }, [selectedProjectId, projectRepositories])

  useEffect(() => {
    if (!selectedProjectId || !selectedRepository) {
      selectedRead.current?.abort()
      setFiles([])
      setSelectedPath('')
      setContent(null)
      setDiff(null)
      return
    }
    const controller = new AbortController()
    selectedRead.current?.abort()
    setLoading(true)
    setError(null)
    setFiles([])
    setSelectedPath('')
    setContent(null)
    setDiff(null)
    setSelectedIndex(-1)
    void fetchRepositoryFiles(
      selectedProjectId,
      selectedRepository.id,
      mode,
      controller.signal,
    )
      .then((result) => {
        if (controller.signal.aborted) return
        setFiles(result.files)
        setFilesTruncated(result.truncated)
      })
      .catch((caught) => {
        if (!controller.signal.aborted) {
          setError(errorMessage(caught, 'Repository files could not be loaded'))
        }
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [mode, selectedProjectId, selectedRepository])

  useEffect(() => {
    setComments([])
    setCommentAnchor(null)
    setReceipt(null)
    setSendError(null)
    retainedCommand.current = null
  }, [selectedProjectId, selectedRepository?.id])

  const tree = useMemo(() => buildRepositoryTree(files), [files])
  const visibleFiles = useMemo(() => {
    const current =
      mode === 'review'
        ? files
        : query
          ? files
          : visibleRepositoryFiles(tree, expanded)
    return fuzzyRepositoryFiles(current, query)
  }, [expanded, files, mode, query, tree])
  const projectAssignments = useMemo(
    () =>
      assignments.filter(
        (assignment) =>
          assignment.project_id === selectedProjectId &&
          assignment.lifecycle === 'active' &&
          assignment.attempt.lifecycle === 'active',
      ),
    [assignments, selectedProjectId],
  )

  useEffect(() => {
    setAssignmentId((current) => {
      const target = activeAssignmentId(activeTarget)
      if (
        target &&
        projectAssignments.some((assignment) => assignment.id === target)
      ) {
        return target
      }
      if (
        current &&
        projectAssignments.some((assignment) => assignment.id === current)
      ) {
        return current
      }
      return projectAssignments[0]?.id ?? ''
    })
  }, [activeTarget, projectAssignments])

  useEffect(() => {
    if (filterOpen) filterRef.current?.focus()
  }, [filterOpen])

  useEffect(() => () => selectedRead.current?.abort(), [])

  const openFile = async (file: RepositoryFile) => {
    if (!selectedRepository) return
    selectedRead.current?.abort()
    const controller = new AbortController()
    selectedRead.current = controller
    setSelectedPath(file.path)
    setContent(null)
    setDiff(null)
    setError(null)
    setSelectedHunk(0)
    setCommentAnchor(null)
    try {
      if (mode === 'browse') {
        const next = await fetchRepositoryFileContent(
          selectedProjectId,
          selectedRepository.id,
          file.path,
          controller.signal,
        )
        if (!controller.signal.aborted) setContent(next)
      } else {
        const next = await fetchRepositoryDiff(
          selectedProjectId,
          selectedRepository.id,
          file.path,
          file.previous_path,
          controller.signal,
        )
        if (!controller.signal.aborted) setDiff(next)
      }
    } catch (caught) {
      if (!controller.signal.aborted) {
        setError(errorMessage(caught, 'Selected file could not be loaded'))
      }
    }
  }

  const handleListKeyDown = (
    event: ReactKeyboardEvent<HTMLDivElement>,
  ) => {
    if (event.key === '/' && !filterOpen) {
      event.preventDefault()
      event.stopPropagation()
      setFilterOpen(true)
      return
    }
    if (event.key === 'Escape' && filterOpen) {
      event.preventDefault()
      event.stopPropagation()
      setFilterOpen(false)
      setQuery('')
      window.requestAnimationFrame(() => listRef.current?.focus())
      return
    }
    if (event.key === 'j' || event.key === 'k') {
      event.preventDefault()
      setSelectedIndex((current) =>
        moveRepositorySelection(
          current,
          visibleFiles.length,
          event.key === 'j' ? 1 : -1,
        ),
      )
      return
    }
    if (event.key === 'Enter' && selectedIndex >= 0) {
      if (event.target !== event.currentTarget) return
      event.preventDefault()
      const file = visibleFiles[selectedIndex]
      if (file) void openFile(file)
    }
  }

  const startComment = (
    startLine: number | null,
    endLine: number | null,
    snippet: string,
  ) => {
    if (!selectedRepository || !selectedPath) return
    setCommentAnchor({
      endLine,
      path: selectedPath,
      repositoryId: selectedRepository.id,
      rootPath: selectedRepository.root_path,
      snippet,
      startLine,
    })
    setCommentBody('')
  }

  const queueComment = (event: FormEvent) => {
    event.preventDefault()
    const body = commentBody.trim()
    if (!commentAnchor || !body) return
    setComments((current) => [...current, { ...commentAnchor, body }])
    retainedCommand.current = null
    setCommentAnchor(null)
    setCommentBody('')
    setReceipt(null)
    setSendError(null)
  }

  const sendComments = async () => {
    const assignment = projectAssignments.find(
      (candidate) => candidate.id === assignmentId,
    )
    if (!assignment || comments.length === 0 || sendBusy) return
    const text = buildRepositoryReviewPrompt(comments)
    const retained =
      retainedCommand.current?.assignmentId === assignment.id &&
      retainedCommand.current.text === text
        ? retainedCommand.current
        : {
            assignmentId: assignment.id,
            commandId: crypto.randomUUID(),
            text,
          }
    retainedCommand.current = retained
    setSendBusy(true)
    setSendError(null)
    try {
      const acknowledgement = await sendAssignmentPrompt(
        assignment.project_id,
        assignment.id,
        {
          actor: 'local-user',
          attempt_id: assignment.attempt.id,
          command_id: retained.commandId,
          expected_assignment_version: assignment.version,
          expected_attempt_version: assignment.attempt.version,
          text,
        },
      )
      setReceipt(acknowledgement)
      setComments([])
      retainedCommand.current = null
    } catch (caught) {
      setSendError(errorMessage(caught, 'Prompt acknowledgement failed'))
    } finally {
      setSendBusy(false)
    }
  }

  const sortedProjects = [...projects].sort(
    (left, right) =>
      Number(!(repositories[left.id]?.repositories.length ?? 0)) -
        Number(!(repositories[right.id]?.repositories.length ?? 0)) ||
      left.name.localeCompare(right.name),
  )

  return (
    <section
      aria-label={activeTarget ? `Files for ${label}` : 'Repository files'}
      className="repository-files-workspace"
    >
      <header className="repository-files-toolbar">
        <label>
          <span>Project</span>
          <select
            aria-label="Files project"
            onChange={(event) => setSelectedProjectId(event.target.value)}
            value={selectedProjectId}
          >
            <option value="">Choose a project</option>
            {sortedProjects.map((project) => (
              <option key={project.id} value={project.id}>
                {project.name}
                {repositories[project.id]?.repositories.length
                  ? ''
                  : ' — no repository'}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span>Repository</span>
          <select
            aria-label="Files repository"
            disabled={projectRepositories.length === 0}
            onChange={(event) =>
              setSelectedRepositoryId(event.target.value)
            }
            value={selectedRepository?.id ?? ''}
          >
            {projectRepositories.map((repository) => (
              <option key={repository.id} value={repository.id}>
                {repository.root_path.split('/').filter(Boolean).at(-1) ??
                  repository.root_path}
              </option>
            ))}
          </select>
        </label>
        <div aria-label="File mode" className="segmented-control" role="group">
          <button
            aria-pressed={mode === 'browse'}
            onClick={() => setMode('browse')}
            type="button"
          >
            Browse
          </button>
          <button
            aria-pressed={mode === 'review'}
            onClick={() => setMode('review')}
            type="button"
          >
            Review
          </button>
        </div>
      </header>
      {!selectedProjectId ? (
        <div className="repository-files-empty" role="status">
          <Folder aria-hidden="true" size={24} />
          <strong>Choose a project</strong>
          <span>Projects with linked repositories are listed first.</span>
        </div>
      ) : projectRepositories.length === 0 ? (
        <div className="repository-files-empty" role="status">
          <GitBranch aria-hidden="true" size={24} />
          <strong>Link a repository</strong>
          <span>Repository identity must be stored on the project first.</span>
          <button
            className="command-button"
            onClick={() => onOpenProjectRepositories(selectedProjectId)}
            type="button"
          >
            Link a repository
          </button>
        </div>
      ) : (
        <div className="repository-files-layout">
          <aside className="repository-files-list">
            <header>
              <span>
                <strong>{mode === 'browse' ? 'Files' : 'Changes'}</strong>
                <small>
                  {files.length}
                  {filesTruncated ? '+ · truncated' : ''}
                </small>
              </span>
              <button
                aria-label="Filter repository files"
                className="icon-button"
                onClick={() => setFilterOpen(true)}
                title="Filter files (/)"
                type="button"
              >
                <Search aria-hidden="true" size={14} />
              </button>
            </header>
            {filterOpen ? (
              <label className="repository-files-filter">
                <Search aria-hidden="true" size={13} />
                <input
                  aria-label="Filter repository files"
                  onChange={(event) => {
                    setQuery(event.target.value)
                    setSelectedIndex(-1)
                  }}
                  placeholder="Fuzzy filter"
                  ref={filterRef}
                  type="search"
                  value={query}
                />
                <button
                  aria-label="Close file filter"
                  className="icon-button"
                  onClick={() => {
                    setFilterOpen(false)
                    setQuery('')
                    listRef.current?.focus()
                  }}
                  type="button"
                >
                  <X aria-hidden="true" size={13} />
                </button>
              </label>
            ) : null}
            <div
              aria-label={`${mode} repository file list`}
              className="repository-files-list__rows"
              onKeyDown={handleListKeyDown}
              ref={listRef}
              role="listbox"
              tabIndex={0}
            >
              {loading ? <p role="status">Loading files…</p> : null}
              {error ? <p className="dialog-error" role="alert">{error}</p> : null}
              {!loading && !error && files.length === 0 ? (
                <p role="status">
                  {mode === 'review' ? 'No working tree changes' : 'No files'}
                </p>
              ) : null}
              {mode === 'browse' && !query ? (
                <RepositoryTreeRows
                  expanded={expanded}
                  nodes={tree}
                  onOpen={openFile}
                  onToggle={(path) =>
                    setExpanded((current) => {
                      const next = new Set(current)
                      if (next.has(path)) next.delete(path)
                      else next.add(path)
                      return next
                    })
                  }
                  selectedPath={
                    visibleFiles[selectedIndex]?.path ?? selectedPath
                  }
                />
              ) : (
                visibleFiles.map((file, index) => (
                  <RepositoryFileRow
                    file={file}
                    key={file.path}
                    onOpen={() => void openFile(file)}
                    selected={
                      file.path === selectedPath || index === selectedIndex
                    }
                  />
                ))
              )}
            </div>
            <footer>j/k move · Enter open · / filter</footer>
          </aside>
          <main className="repository-file-pane">
            {!selectedPath ? (
              <div className="repository-files-empty" role="status">
                <File aria-hidden="true" size={22} />
                <strong>Select a file</strong>
                <span>{selectedRepository?.root_path}</span>
              </div>
            ) : mode === 'browse' ? (
              <FileContentView content={content} />
            ) : (
              <DiffView
                diff={diff}
                hunkRefs={hunkRefs}
                onComment={startComment}
                onSelectHunk={setSelectedHunk}
                selectedHunk={selectedHunk}
              />
            )}
          </main>
        </div>
      )}
      {commentAnchor ? (
        <form className="repository-comment-composer" onSubmit={queueComment}>
          <header>
            <strong>Comment on {commentAnchor.path}</strong>
            <button
              aria-label="Cancel review comment"
              className="icon-button"
              onClick={() => setCommentAnchor(null)}
              type="button"
            >
              <X aria-hidden="true" size={14} />
            </button>
          </header>
          <pre>{commentAnchor.snippet}</pre>
          <textarea
            aria-label="Review comment"
            autoFocus
            onChange={(event) => setCommentBody(event.target.value)}
            placeholder="What should the agent change?"
            required
            value={commentBody}
          />
          <button className="command-button" type="submit">
            Queue comment
          </button>
        </form>
      ) : null}
      {comments.length > 0 ? (
        <section
          aria-label="Queued review comments"
          className="repository-review-queue"
        >
          <header>
            <strong>{comments.length} queued</strong>
            <label>
              <span>Assignment</span>
              <select
                aria-label="Review target assignment"
                onChange={(event) => {
                  setAssignmentId(event.target.value)
                  retainedCommand.current = null
                }}
                value={assignmentId}
              >
                {projectAssignments.map((assignment) => (
                  <option key={assignment.id} value={assignment.id}>
                    {assignment.role} · {assignment.profile_name}
                  </option>
                ))}
              </select>
            </label>
          </header>
          {comments.map((comment, index) => (
            <div key={`${comment.path}:${comment.startLine}:${index}`}>
              <span>{comment.path}:{comment.startLine ?? 'hunk'}</span>
              <button
                aria-label={`Remove comment ${index + 1}`}
                className="icon-button"
                onClick={() => {
                  setComments((current) =>
                    current.filter((_, candidate) => candidate !== index),
                  )
                  retainedCommand.current = null
                }}
                type="button"
              >
                <X aria-hidden="true" size={13} />
              </button>
            </div>
          ))}
          <button
            className="command-button"
            disabled={!assignmentId || sendBusy}
            onClick={() => void sendComments()}
            type="button"
          >
            <Send aria-hidden="true" size={14} />
            {sendBusy ? 'Sending…' : 'Send review'}
          </button>
          {!assignmentId ? (
            <p className="dialog-error" role="alert">
              No active assignment can receive this review.
            </p>
          ) : null}
          {sendError ? <p className="dialog-error" role="alert">{sendError}</p> : null}
        </section>
      ) : null}
      {receipt ? (
        <p className="repository-review-receipt" role="status">
          Sent via prompt acknowledgement · {receipt.command_id}
        </p>
      ) : null}
    </section>
  )
}

function RepositoryTreeRows({
  expanded,
  nodes,
  onOpen,
  onToggle,
  selectedPath,
  depth = 0,
}: {
  depth?: number
  expanded: ReadonlySet<string>
  nodes: RepositoryTreeNode[]
  onOpen: (file: RepositoryFile) => void
  onToggle: (path: string) => void
  selectedPath: string
}) {
  return nodes.map((node) =>
    node.type === 'directory' ? (
      <div key={node.path}>
        <button
          aria-expanded={expanded.has(node.path)}
          className="repository-tree-directory"
          onClick={() => onToggle(node.path)}
          style={{ paddingLeft: `${8 + depth * 14}px` }}
          type="button"
        >
          {expanded.has(node.path) ? (
            <ChevronDown aria-hidden="true" size={13} />
          ) : (
            <ChevronRight aria-hidden="true" size={13} />
          )}
          <Folder aria-hidden="true" size={13} />
          <span>{node.name}</span>
        </button>
        {expanded.has(node.path) ? (
          <RepositoryTreeRows
            depth={depth + 1}
            expanded={expanded}
            nodes={node.children}
            onOpen={onOpen}
            onToggle={onToggle}
            selectedPath={selectedPath}
          />
        ) : null}
      </div>
    ) : node.file ? (
      <RepositoryFileRow
        depth={depth}
        file={node.file}
        key={node.path}
        onOpen={() => onOpen(node.file as RepositoryFile)}
        selected={node.path === selectedPath}
      />
    ) : null,
  )
}

function RepositoryFileRow({
  depth = 0,
  file,
  onOpen,
  selected,
}: {
  depth?: number
  file: RepositoryFile
  onOpen: () => void
  selected: boolean
}) {
  return (
    <button
      aria-selected={selected}
      className="repository-file-row"
      data-state={file.state}
      onClick={onOpen}
      role="option"
      style={{ paddingLeft: `${22 + depth * 14}px` }}
      type="button"
    >
      <FileDiff aria-hidden="true" size={13} />
      <span>{file.path.split('/').at(-1)}</span>
      <small>
        {file.state}
        {file.staged ? ' · staged' : ''}
        {file.unstaged ? ' · unstaged' : ''}
      </small>
    </button>
  )
}

function FileContentView({
  content,
}: {
  content: RepositoryFileContent | null
}) {
  if (!content) return <div className="repository-files-empty">Loading file…</div>
  if (content.unavailable) {
    return <div className="repository-files-empty">File is unavailable.</div>
  }
  if (content.binary) {
    return <div className="repository-files-empty">Binary file · preview unavailable</div>
  }
  return (
    <section className="repository-content">
      <header>
        <strong>{content.path}</strong>
        {content.truncated ? <small>Truncated</small> : null}
      </header>
      <ol>
        {(content.content ?? '').split('\n').map((line, index) => (
          <li key={index}><code>{line || ' '}</code></li>
        ))}
      </ol>
    </section>
  )
}

export function DiffView({
  diff,
  hunkRefs,
  onComment,
  onSelectHunk,
  selectedHunk,
}: {
  diff: RepositoryDiff | null
  hunkRefs: MutableRefObject<Array<HTMLElement | null>>
  onComment: (
    startLine: number | null,
    endLine: number | null,
    snippet: string,
  ) => void
  onSelectHunk: (index: number) => void
  selectedHunk: number
}) {
  if (!diff) return <div className="repository-files-empty">Loading diff…</div>
  if (diff.unavailable) {
    return <div className="repository-files-empty">Diff is unavailable.</div>
  }
  if (diff.binary) {
    return <div className="repository-files-empty">Binary change · diff unavailable</div>
  }
  if (diff.hunks.length === 0) {
    return <div className="repository-files-empty">No diff for this file.</div>
  }
  return (
    <section
      className="repository-diff"
      onKeyDown={(event) => {
        if (event.key !== 'n' && event.key !== 'p') return
        event.preventDefault()
        const next = Math.max(
          0,
          Math.min(
            diff.hunks.length - 1,
            selectedHunk + (event.key === 'n' ? 1 : -1),
          ),
        )
        onSelectHunk(next)
        hunkRefs.current[next]?.scrollIntoView({ block: 'start' })
      }}
      tabIndex={0}
    >
      <header>
        <span>
          <strong>{diff.path}</strong>
          {diff.previous_path ? <small>from {diff.previous_path}</small> : null}
        </span>
        <small>
          {diff.hunks.length} hunks{diff.truncated ? ' · truncated' : ''}
        </small>
      </header>
      {diff.hunks.map((hunk, hunkIndex) => (
        <article
          className="repository-diff-hunk"
          data-selected={selectedHunk === hunkIndex}
          key={`${hunk.old_start}:${hunk.new_start}:${hunkIndex}`}
          ref={(node) => {
            hunkRefs.current[hunkIndex] = node
          }}
        >
          <header>
            <code>
              -{hunk.old_start},{hunk.old_lines} +{hunk.new_start},{hunk.new_lines}
            </code>
            <button
              className="secondary-button"
              onClick={() =>
                onComment(
                  hunk.new_start,
                  hunk.new_start + Math.max(0, hunk.new_lines - 1),
                  hunk.lines.slice(0, 20).map(diffLineSnippet).join('\n'),
                )
              }
              type="button"
            >
              <MessageSquareText aria-hidden="true" size={12} />
              Comment
            </button>
          </header>
          {hunk.lines.map((line, lineIndex) => (
            <DiffLine
              key={`${line.old_line}:${line.new_line}:${lineIndex}`}
              line={line}
              onComment={() => {
                const lineNumber = line.new_line ?? line.old_line
                onComment(lineNumber, lineNumber, diffLineSnippet(line))
              }}
            />
          ))}
        </article>
      ))}
      <footer>n/p next or previous hunk</footer>
    </section>
  )
}

function DiffLine({
  line,
  onComment,
}: {
  line: RepositoryDiffLine
  onComment: () => void
}) {
  const oldLine = String(line.old_line ?? '').padStart(5)
  const newLine = String(line.new_line ?? '').padStart(5)
  return (
    <button
      aria-label={`Comment on line ${line.new_line ?? line.old_line ?? ''}`}
      className="repository-diff-line"
      data-kind={line.kind}
      onClick={onComment}
      type="button"
    >
      <code>{`${oldLine} ${newLine} ${diffLineSnippet(line)}`}</code>
    </button>
  )
}

function diffLineSnippet(line: RepositoryDiffLine) {
  const prefix =
    line.kind === 'addition' ? '+' : line.kind === 'deletion' ? '-' : ' '
  return `${prefix}${line.content}`
}

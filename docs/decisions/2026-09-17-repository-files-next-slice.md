# ADR: Repository-backed Files next slice

- Status: Proposed
- Date: 2026-09-17

## Context

Yard now has one durable `project_repositories` association for project-owned
checkout roots. The previous Files surface inferred repository identity from a
Herdr pane working directory, allowed a selected directory to diverge from the
terminal identity, eagerly fetched review patches, and parsed unified diffs in
the browser. That surface is disabled until the durable association is the only
authorization boundary.

The legacy runtime/session changes and file-content HTTP routes are not
registered while repository-scoped replacements are absent. Requests to those
routes fail closed with `404 Not Found`.

This ADR defines the next reviewable slice. It does not add a repository service,
a generic path framework, or a second feedback/history system.

## Decision

Implement one project-scoped repository surface backed directly by the existing
`YardStore` association. A Herdr CWD may be shown only as a discovery suggestion;
it never selects, authorizes, or persists a repository.

### Repository management

Expose only these direct routes:

- `GET /api/v1/projects/{project_id}/repositories`
- `POST /api/v1/projects/{project_id}/repositories`
- `PUT /api/v1/projects/{project_id}/repositories/{repository_id}`
- `DELETE /api/v1/projects/{project_id}/repositories/{repository_id}`

Create and relink accept only one absolute root path; caller-supplied identity
fields are rejected. The server canonicalizes the existing directory and
derives the Git top-level and common directory with a bounded two-second Git
check. It stores those canonical identities, normalizes symlink aliases, and
rejects missing roots, non-worktrees, duplicate project roots, paths outside the
selected checkout, and relinks to a different Git common directory.

Repository reads and mutations are scoped to an active owning project.
Cross-project IDs and repositories belonging to archived or deleted projects
are inaccessible. Project archive and delete transactions explicitly purge
repository associations, including stale associations encountered during an
idempotent lifecycle replay. Every Files response includes the selected
repository ID and root explicitly.

### Metadata-only file lists

Expose one list route:

- `GET /api/v1/projects/{project_id}/repositories/{repository_id}/files?mode=browse|review`

Browse returns bounded tracked and untracked path metadata. Review returns bounded
changed-path metadata with status, previous path, staged, and unstaged flags. The
list performs one repository lookup and at most two bounded Git subprocesses,
never one query or process per file. It returns truncation metadata rather than
eager file content or patches.

### Lazy selected-file reads

Expose only selected-file reads:

- `GET /api/v1/projects/{project_id}/repositories/{repository_id}/files/content?path=...`
- `GET /api/v1/projects/{project_id}/repositories/{repository_id}/diff?path=...`

Content returns bounded text plus binary, truncated, and unavailable metadata.
Diff returns server-generated structured hunks: hunk ranges and ordered lines
with kind, old line, new line, and content. The client does not parse unified
diff text.

The selected read handles deleted, sparse, binary, oversized, unborn, renamed,
untracked, staged-only, unstaged-only, and staged-plus-unstaged files without
failing the file list. Git runs with external diff and text conversion disabled,
optional locks and fsmonitor disabled, bounded output, and a two-second timeout.

Before a selected-file read, the server revalidates the stored canonical root
and Git common-directory identity and fails closed if either has changed.

### UI and feedback

Replace the temporary unavailable state in the existing Files surface; do not
create another artifact, file, or review shell. Repository selection is explicit
at project scope. Browse fetches only Browse metadata. Review metadata is fetched
only after switching modes. Content or hunks load only after selecting a file.

Do not use `yard:files-directory:*`, terminal-derived repository inference, or
client unified-diff parsing. Review feedback for an assignment uses the existing
assignment-attempt prompt command and its current optimistic versions.

### Regression budgets

- List 200 changed-file metadata records in under 100 ms in the fixture.
- Render a selected 2,000-line structured diff in under 200 ms.
- Use one repository database lookup per request and no per-record fetching.
- Use at most two Git subprocesses for a metadata list and three for a selected
  diff, each with a two-second timeout and bounded output.
- Keep Browse and Review usable without horizontal document overflow at desktop
  and 390-pixel mobile widths.

## Verification

The slice requires migration/domain/store tests, server route and file-state
tests, subprocess-count and performance coverage, frontend unit coverage for
structured hunks, and focused Playwright coverage for repository linking,
Browse, Review, assignment feedback, desktop, and mobile.

## Non-goals

- Repository discovery or authorization from runtime CWD.
- A generic repository framework or background index.
- Eager patch generation or per-file Git subprocesses.
- A parallel review comment or message history.
- Multiple competing file/review surfaces.

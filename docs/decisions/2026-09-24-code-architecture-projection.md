# ADR: Code architecture projection

- Status: Proposed
- Date: 2026-09-24

## Context

Project territories previously generated decorative buildings from assignment,
artifact, completion, and token counts. Those shapes did not describe the
project's code and changed position when unrelated work records changed.

Projects already own durable repository associations with canonical Git root
and common-directory identities. The map can use those associations as the
only filesystem authorization boundary.

## Decision

Expose `GET /api/v1/projects/{project_id}/architecture`. The response uses
portable package vocabulary:

- project architecture;
- one repository architecture per durable association;
- nodes with stable repository, ecosystem, and relative-manifest identity;
- optional directed edges only for declared dependencies whose target is
  another detected node.

The initial analyzers are exactly:

- Cargo manifests: virtual workspaces are containers; manifests with a
  `[package]` table are nodes. Explicit `[lib]` and `[[bin]]` targets classify
  libraries and applications; otherwise the node is a package.
- npm manifests: each bounded `package.json`, including workspace packages, is
  a node. A declared `bin` classifies an application; otherwise it is a
  package.

Other ecosystems return an explicit unsupported repository status. Malformed or
partially unreadable repositories retain successfully detected nodes and report
partial or error status.

Before each cold scan, the server revalidates the stored canonical checkout root
and Git common-directory identity. Scanning uses bounded directory traversal,
does not follow symlinks, rejects escaping symlinks, and skips `.git`,
`node_modules`, `target`, `dist`, `build`, `out`, and `vendor`.

Limits are:

- 16 repositories per response;
- depth 16;
- 10,000 directory entries;
- 256 KiB per manifest;
- 1,000 nodes;
- 4,000 edges;
- 64 diagnostics per repository;
- 2 MiB serialized response.

Any exceeded limit sets `truncated`. Results use a 30-second in-memory cache.
Expired results are returned as `stale` while one bounded background refresh
runs. `scanned_at_unix_ms`, `stale`, and `truncated` remain visible to clients.
No architecture data is persisted.

In the projected map, a project territory contains one district per repository
association and one non-interactive building per architecture node. Districts
show ready, unsupported, partial, error, and truncation states. Empty projects
distinguish loading, unavailable scans, no repository associations, and no
supported nodes.

District placement depends only on repository identity ordering. Building
geometry depends only on stable node identity and its district rectangle, so
adding a node does not move existing buildings. Buildings and districts do not
accept pointer events; worker selection, dragging, allocation, navigation, and
territory interactions remain on their existing layers.

Assignment, artifact, completion, and token counts no longer affect buildings.
Architecture dependency edges are returned but are not drawn as operational
routes.

## Consequences

The projection truthfully reflects declared package architecture with bounded
filesystem cost and explicit uncertainty. Hash placement can overlap in very
large districts, but it preserves identity stability without persistence or a
graph-layout dependency.

The TOML parser is the only new dependency and is pinned exactly because robust
Cargo manifest parsing is not available in the standard library.

## Deferred

- additional ecosystems;
- lockfile or resolved dependency graphs;
- source, AST, LSP, or runtime-service inference;
- architecture dependency visualization;
- collision-free persistent building slots;
- repository-management UI or top-bar controls.

# ADR: Portable Agent Profile Bundle v1

- Date: 2026-09-24
- Status: Proposed

## Context

Yard already stores immutable worker-profile revisions and a portable
`AgentProfile` manifest, but launches still consume the flat compatibility
projection. Raw tool, skill, and MCP strings imply injection that most launch
paths cannot perform. Manifest artifacts also carried digests without package
bytes, so Yard could not verify or materialize them.

## Decision

The implemented v1 bundle is an immutable profile revision containing:

- the existing `yard.dev/agent-profile/v1alpha1` manifest;
- optional UTF-8 package files with relative paths, media types, and verified
  SHA-256 digests;
- the manifest's portable role, ordered instructions, required and optional
  capabilities, distinct skill/tool/MCP components, credential-slot
  declarations, policies, runtime constraints, provenance, and namespaced
  extensions; and
- a stored validation report that preserves unsupported imported intent
  without granting runtime capabilities.

Unknown manifest and extension fields remain round-trippable. Supported v1
package media are Markdown, plain text, and JSON. Each file is limited to 256
KiB, the bundle to 2 MiB, and the existing 256-artifact limit applies.
Absolute, parent, Windows-style, duplicate, undeclared, oversized,
digest-mismatched, secret-bearing, and unsupported-media inputs are rejected.
The JSON import surface cannot express symlinks, devices, or other special
files.

`agent_profile_files` stores package bytes beside the immutable profile
revision. Existing revisions backfill with no package bytes. Legacy non-empty
component lists remain compatibility stubs: the two embedded Yard skills are
recognized, while unknown legacy components remain explicit launch failures.
Worker-profile edits preserve imported package bytes and opaque extensions.

## Compile and negotiation

Runtime lowering lives in `yard-server`, not `yard-domain`. A deterministic
dry run returns:

- provider arguments;
- each instruction, skill, tool, MCP server, and permission policy with
  `supported`, `degraded`, `approval_required`, or `unsupported` status;
- generated relative files and their digests/provenance;
- effective permission intent;
- missing capabilities, warnings, and required approvals; and
- one compatible/blocked result.

Import never resolves credentials, expands permissions, bypasses sandboxing,
enables network access, or authorizes token spend. Imported full-access intent
is preserved under the `dev.yard.import` extension but normalized to
`runtime-default`, and dry run reports the unresolved intent as
`approval_required`. Resolving it requires an explicit Worker Profile policy
edit in Yard, which creates a new immutable revision and clears the import
marker. A full-access policy authored in Yard remains supported: confirmed
allocation is the user's explicit approval, so the plan and durable audit
record the permission and retain the existing provider argument. Required
unsupported behavior blocks launch; optional unsupported behavior remains a
warning unless it originated from a legacy raw list that previously claimed
injection.

Each wired launch records the complete plan in `profile_launch_audits` before
runtime mutation. Runtime status is not used as completion proof.

## Provider lowering

Codex 0.154 lowers project skills to
`.agents/skills/<skill>/SKILL.md`. Claude lowers the same portable bytes to
`.claude/skills/<skill>/SKILL.md`; workspace instruction references are
reported as degraded because Yard composes them into the launch prompt.
Providers without a tested project skill surface report unsupported.

Primary runtime references: [Codex skill discovery][codex-skills] and
[Claude Code skills][claude-skills].

The dry run lists every generated repository path before launch. Managed skill
files are written atomically by rename under the selected launch working
directory at `.agents/skills/<skill>/SKILL.md` or
`.claude/skills/<skill>/SKILL.md`. Every existing path component is checked
with `symlink_metadata`; links and unsupported file types fail closed.
Existing identical files are reused, conflicting files are refused, and
cleanup removes only unchanged files whose digest matches the launch plan.
The durable database launch audit is the receipt; no repository-local receipt
file is written.

## Built-in orchestrator kit

Yard packages relocatable `herdr-orchestration` and `herdr-cli` Agent Skill
directories under the repository `skills/` tree. The text was adapted from
the local source to remove harness-specific reading terminology, stale
version-specific behavior, and machine assumptions. It contains no absolute
developer-home paths.

The built-in Orchestrator profile template requests both skills. Codex and
Claude launch plans materialize them through native project discovery without
requiring `CLAUDE.md`. No live model session is launched by tests.

## API and UI

`POST /api/v1/agent-profiles/{profile_id}/dry-run` accepts an immutable
profile revision and performs no filesystem or runtime mutation. The profile
editor no longer exposes raw tool, skill, or MCP text fields. It preserves
existing compatibility data, shows packaged skills, and previews the
server-compiled launch plan, including exact generated paths, for a saved
revision.

## Migration and scope

Migration 0032 adds immutable package files and launch audits without changing
existing profile identity or revision pins. Existing raw lists are preserved.
Known embedded skills become materializable; other entries fail explicitly at
launch rather than disappearing.

This slice wires compile, audit, and materialization into confirmed worker
allocation, replacement-runtime allocation, handoff provisioning, and both
profile-backed project creation paths. Other dedicated launch paths retain
their existing strict rejection until they adopt the same compiler.

Deferred ecosystems include archive/directory upload, binary package media,
provider-native plugins/hooks/subagents, MCP transport and credential
resolution, Kiro/Hermes lowering, a dedicated import-approval UI, and
automatic managed-file cleanup.

[codex-skills]: https://developers.openai.com/plugins/build/skills
[claude-skills]: https://code.claude.com/docs/en/skills

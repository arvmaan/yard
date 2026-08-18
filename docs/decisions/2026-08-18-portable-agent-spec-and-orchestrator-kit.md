# Proposed ADR: Portable Agent Specification and Orchestrator Kit

- Date: 2026-08-18
- Status: Proposed
- Owners: Yard profile and orchestrator lifecycle
- Related research:
  [2026-08-18 portable profiles and orchestrator replacement](../research/2026-08-18-portable-profiles-and-orchestrator-replacement.md)

## Context

`WorkerProfile` is currently a versioned Yard record, but its spec is coupled
to one runtime adapter and launch model. It stores `runtime_adapter`,
`provider`, `model`, policy strings, and lists for tools, skills, and MCP
servers. The Herdr adapter accepts only a narrow subset: non-empty capability
lists are rejected, model and full-access arguments are mapped only for Codex
and Claude, and `instructions_ref` is rendered into prompt text rather than
resolved.

Provider configuration is not interchangeable. Claude Code, Codex, Kiro, and
Hermes differ in instruction discovery and precedence, skill and plugin
locations, subagent definitions, hooks, policy enforcement, and supported
surfaces. Yard must not represent a provider-specific capability as a
universal feature merely because several providers use a similar name.

Yard-created project orchestrators also need a usable orchestration kit. A
prompt that names a skill is insufficient unless the skill and its companion
Herdr command reference are available in the new orchestrator's discovery
scope. The package must be relocatable and must preserve explicit opt-in for
additional agent token spend.

## Decision

Yard will introduce a bundle-based, versioned `AgentProfile` format. The
format has three deliberately separate layers:

1. A portable semantic core for role, instructions, requested capabilities,
   components, and policy intent.
2. Namespaced adapter extensions for provider- or runtime-specific settings.
3. Package artifacts containing instructions, skills, plugin data, and other
   relative resources with hashes and provenance.

Yard will retain its existing profile ID and immutable revision number.
Neither is the portable format version. A profile revision pins one canonical
imported bundle and its validation result.

### Proposed v1alpha1 Shape

```yaml
apiVersion: yard.dev/agent-profile/v1alpha1
kind: AgentProfile
metadata:
  name: Project orchestrator
  packageVersion: 1.0.0
  provenance:
    importedFrom: yard-worker-profile
    importedAt: 2026-08-18T00:00:00Z
spec:
  role:
    default: orchestrator
  instructions:
    - id: project-instructions
      source:
        kind: workspace
        path: AGENTS.md
      required: false
  capabilities:
    required:
      - id: filesystem.workspace.read
        version: 1
      - id: filesystem.workspace.write
        version: 1
      - id: process.shell
        version: 1
    optional:
      - id: agent.skills
        version: 1
        allowDegraded: false
      - id: tool.mcp.client
        version: 1
        allowDegraded: false
  components:
    skills:
      - id: herdr-orchestration
        source:
          kind: bundle
          path: skills/herdr-orchestration
        required: true
    tools: []
    mcpServers: []
  policies:
    isolation: project-workspace
    sandbox: runtime-default
    permissions: runtime-default
    completion: explicit-receipt
  extensions:
    dev.yard.herdr:
      apiVersion: yard.dev/adapters/herdr/v1alpha1
      agentKind: codex
    com.openai.codex:
      apiVersion: yard.dev/providers/codex/v1alpha1
      model: gpt-5.6
artifacts:
  - path: skills/herdr-orchestration/SKILL.md
    mediaType: text/markdown
    sha256: "<digest>"
```

This is an envelope proposal, not a claim that every example capability is
available in the current runtime.

### Portable Core

The portable core may express only semantic intent that an adapter can
negotiate:

- default role and ordered instruction sources;
- required and optional capabilities with version constraints;
- skill, tool, plugin, and MCP component references;
- isolation, sandbox, permission, and completion policy intent;
- relative package artifacts and provenance.

Model IDs, provider command arguments, hook event names, subagent file
formats, provider-specific policy keys, and native configuration fragments
belong in extensions. An extension cannot weaken a portable or Yard-enforced
policy. A conflict is a validation error.

### Adapter Extensions

Extension keys use reverse-DNS ownership. Every extension carries its own
`apiVersion`. Importers preserve unknown extension data and artifacts without
interpreting them. Export must not silently omit an unknown extension.

The first implementations should define:

- `dev.yard.herdr` for runtime session, agent kind, and launch behavior;
- `com.anthropic.claude-code`;
- `com.openai.codex`;
- `dev.kiro`;
- `com.nousresearch.hermes`.

Provider adapters own lowering to command arguments, native files, plugin
roots, settings, and instruction order. The portable domain does not build
provider command lines.

### Capability Negotiation

Before save, import, launch, or export-to-provider, Yard evaluates the profile
against an adapter descriptor containing adapter version, provider version,
runtime surface, and supported capability versions. Each request produces one
of:

- `supported`: native semantics satisfy the request;
- `degraded`: an explicit emulation or semantic difference exists;
- `approval_required`: the operation is available only after a separate
  authorization;
- `unsupported`: the adapter cannot satisfy the request.

Every result includes a reason and the provider surface used. A required
capability blocks launch unless it is `supported`, or unless the request
explicitly permits the reported degradation. Optional unsupported
capabilities produce warnings. Negotiation results are persisted with the
launch command for audit and replay; they are not inferred later from runtime
status.

### Skills, Tools, and MCP

Skills follow the public Agent Skills directory convention where possible:
one directory with `SKILL.md` and relative supporting files. The Yard bundle
still versions and hashes the directory because the Agent Skills
specification does not provide a top-level package version.

Tools are typed references, not free-form names. MCP servers are separate
components with transport configuration and credential slots. A skill may
request tools or MCP capabilities, but importing a skill does not
automatically grant them.

Hooks, subagents, plugins, and administrative policies remain distinct
extension capability classes. They must not be flattened into `skills` or
prompt instructions.

### Secrets

Portable bundles never contain resolved credentials, API keys, bearer tokens,
cookies, private environment values, OAuth tokens, or keychain contents.

The schema permits only credential slot declarations, such as an environment
variable name, local secret-store key, OAuth binding name, or command-helper
identifier. Import validates that no value is present. Resolution happens
locally at launch after policy checks. Export emits slot declarations and a
redaction report, never resolved data.

Packages also reject absolute paths, paths escaping the bundle, device files,
and links that resolve outside the package. Artifact size, count, media type,
and digest are validated before extraction.

### Precedence

Yard composes configuration from lowest to highest precedence:

1. provider and runtime defaults;
2. portable profile defaults;
3. the selected adapter extension, for namespaced settings only;
4. project overlay;
5. explicit assignment or launch overrides;
6. administrator policy and Yard safety invariants.

Policy layers intersect; a higher layer cannot weaken a deny or hard safety
constraint. Provider-specific extension data cannot override a portable core
field. Conflicts fail validation instead of relying on object merge order.

Instructions use a separate ordered composition. Yard records each source,
scope, and merge mode, then the adapter adds native provider discovery rules.
The compiled launch plan exposes the effective order and warns when native
files may shadow or replace imported content. Yard does not claim a single
instruction precedence model across providers.

### Validation

Validation has five stages:

1. Structural: known `apiVersion`, required fields, bounds, and types.
2. Package: relative contained paths, digests, media types, and no secret
   values.
3. Semantic: unique IDs, resolvable references, policy consistency, and
   extension ownership.
4. Adapter: capability negotiation and provider-version constraints.
5. Compile dry run: generated arguments and files are deterministic and do
   not escape the owned workspace or declared runtime directory.

Unknown core format versions are rejected. Unknown extensions are preserved
but cannot be selected for launch without an installed adapter. Import and
export reports include errors, warnings, degraded fields, preserved opaque
fields, and redactions.

### Import and Export

The canonical interchange is a directory or archive containing the manifest
and artifacts. JSON and YAML are equivalent manifest serializations after
canonicalization. Yard keeps:

- the original manifest and artifacts for lossless re-export;
- a canonical parsed representation for validation and launch;
- source provider and version;
- package and artifact digests;
- the adapter negotiation report.

Provider importers translate recognized native files into the portable core
and namespaced extensions. Untranslated native content remains as an opaque
artifact with provenance and a warning. Provider exporters fail or report a
degradation for unsupported required behavior; they never silently discard
it.

### Migration from WorkerProfile

Migration is additive and preserves all existing `(profile_id, version)`
foreign keys:

| WorkerProfile field | AgentProfile mapping |
|---|---|
| `name` | `metadata.name` |
| `default_role` | `spec.role.default` |
| `instructions_ref` | ordered workspace instruction source |
| `tools` | typed component stubs requiring negotiation |
| `skills` | skill component stubs requiring negotiation |
| `mcp_servers` | MCP component stubs requiring negotiation |
| `sandbox_policy` | `spec.policies.sandbox` |
| `worktree_policy` | `spec.policies.isolation` |
| `permission_policy` | `spec.policies.permissions` |
| `completion_contract` | `spec.policies.completion` |
| `runtime_adapter` | runtime adapter extension |
| `provider`, `model` | provider extension selected by the runtime adapter |
| `version` | unchanged Yard profile revision, not format version |

A future migration should add a canonical spec blob and validation metadata
per immutable profile revision, backfill every historical revision, and keep
the existing columns as compatibility projections during a dual-read period.
The current REST shape remains available until v2 import/export and launch
paths have round-trip fixtures. Existing unsupported non-empty capability
lists remain unsupported after migration; they become explicit negotiation
failures rather than pretend-portable fields.

### Embedded Project-Orchestrator Kit

Yard will own and embed a versioned kit in the server distribution:

```text
yard-orchestrator-kit/
  manifest.yaml
  skills/
    herdr-orchestration/
      SKILL.md
      scripts/yard-herdr-orchestrate
      templates/lane-prompt.md
      schemas/lane-result.schema.json
    herdr-cli/
      SKILL.md
```

Project creation materializes the verified kit atomically under a
project-relative Yard-managed directory before the orchestrator starts.
Provider adapters expose that directory through their supported skill or
plugin discovery mechanism. The source kit is embedded or installed relative
to the Yard executable; it never references a developer home directory.

The wrapper discovers the repository root, `herdr`, `jq`, supported agent
kinds, and current flags at runtime. It captures Herdr JSON IDs, uses plain
workspaces for read-only lanes, creates isolated worktrees only for write
lanes, polls status, and requires a structured lane result. Internal paths are
relative to the kit or resolved from `git rev-parse`; transient state uses
`$YARD_RUN_DIR` or an XDG-derived directory.

Creating a Herdr worktree or starting an agent does not make that runtime a
Yard project worker. Herdr may create the worktree in a new top-level
workspace, while Yard binds the project to a different workspace. Accepted
ADR-0008 rejects allocating a live worker across that boundary. The embedded
kit must therefore adopt each new runtime into the project-bound workspace
before Yard confirmed allocation:

1. Read fresh Herdr and Yard inventory and resolve the project's currently
   owned Herdr workspace.
2. Create the isolated worktree and start the agent, but do not allocate it.
3. Capture the terminal ID, provider session, process identity and state,
   isolated worktree CWD, source workspace, and current pane/tab topology.
4. If the source differs from the project workspace, run:

   ```sh
   herdr pane move <pane> --workspace <project-workspace> --new-tab \
     --label <label>
   ```

5. Read both inventories again. Require one unique runtime in the project
   workspace with the same terminal ID, provider session, process, and CWD.
   Pane and tab IDs may change during movement and must be recaptured.
6. Require Yard inventory reconciliation to represent that worker in the
   project-bound workspace. Only then may the normal confirmed allocation
   create an active Yard assignment.

Movement and reconciliation fail closed. A failed or ambiguous move triggers
fresh inventory and lookup by the captured stable identity; allocation may
continue only when one unique match is in the target workspace. A match still
in the source workspace requires a deliberate retry, while an absent or
ambiguous match stops the operation. Failure to reconcile stops allocation
without blind recreation or movement. If allocation fails after successful
adoption, retain the observed unassigned worker, reload command versions, and
retry according to allocation idempotency; do not recreate or move it again.
All retained resources remain available for inspection, and any later cleanup
must revalidate captured identity and owned-workspace constraints.

Starting or prompting additional agents requires an explicit, non-persisted
`--approve-agent-spend` grant plus bounded agent and depth limits. Planning
and dry run are the default. The kit does not add dangerous autonomy flags by
default. Cleanup is a separate explicit action and never automatically
deletes transcripts, branches, or useful worktrees.

This policy is instruction- and wrapper-enforced in this lane. Strong runtime
budget enforcement belongs to the separate token-coordination design and is
not changed by this ADR.

## Consequences

- Yard can round-trip provider configuration without pretending all features
  are portable.
- Adapter behavior becomes testable and version-qualified.
- Existing immutable profile revision and assignment pinning remain intact.
- Import/export and kit materialization require new schema, storage, adapter,
  and security work before the UI can advertise portability.
- Provider changes can produce explicit degradation warnings rather than
  silently changing behavior.
- The embedded kit can create real isolated Herdr lanes from a new project
  orchestrator, adopt them into the Yard-owned project workspace without
  replacing their stable runtime identity, and allocate them without local
  absolute paths or implicit token spend.

## Non-Goals

- Standardizing provider-native hooks, subagent formats, or policy languages.
- Storing or transferring secrets.
- Changing token coordination settings.
- Changing completion semantics: explicit evidence-backed receipts remain
  required.
- Implementing orchestrator replacement in this ADR.

# Security

## Supported Version

Yard is pre-release software. Only the current `main` branch receives fixes.

## Trust Boundary

Yard v0.1 is intended for one local operator. It has no user authentication,
authorization, TLS termination, or remote deployment model. The server rejects
non-loopback binds.

Running Yard grants it access to:

- the configured Herdr sessions and their terminals;
- project working directories selected by the operator;
- the Yard SQLite database and managed artifact, coordination, and knowledge
  directories;
- Ghostty when external terminal launch is enabled.

Artifacts are size-limited and verified by digest. HTML previews are sanitized
and sandboxed. Managed knowledge and coordination paths reject symlink
traversal. These controls do not make untrusted local agents safe.

The lifecycle CLI uses a database-scoped, owner-only Unix socket, same-UID peer
credentials, and a random per-launch secret. The process holds a persistent
instance lock for its lifetime. Stored PIDs are diagnostic: `yard stop`
requests shutdown through the authenticated control channel and never signals
a PID loaded from lifecycle metadata.

Yard's macOS Terminal handoff has the same local single-owner boundary. Yard
creates missing cache directories as `0700` and accepts only current-UID-owned
cache ancestry with no group/other write permission. Owner-only `0700` handoff
files and no-follow ancestry checks protect handoff contents from other OS
users and reject accidental symlink or permissive-path configuration.
Descriptor-relative creation, pre-launch identity checks, and
quarantine-before-delete cleanup narrow ordinary Yard-owned races. They do not defend against an actively malicious process
running as the same UID, which can already control Yard and Terminal, access
Herdr configuration and credentials, and modify the user's files. Terminal.app
must resolve a pathname after launch, so Yard does not claim an atomic binding
between its final descriptor checks and Terminal's later lookup.

## Live Harness Warning

`scripts/live-v1-acceptance.sh` starts authenticated Codex agents with
`--yolo`. It can consume quota and execute commands without interactive
approval inside its temporary repositories. Do not run it with credentials or
host access you are unwilling to expose to those agents.

## Data Handling

Stop Yard before backing up or restoring its SQLite database and sibling
managed directories. Do not commit `.yard/`, SQLite files, credentials,
terminal output, or generated knowledge containing secrets.

## Reporting

Report vulnerabilities through a private GitHub security advisory for
`arvmaan/yard`. Do not include exploit details in a public issue.

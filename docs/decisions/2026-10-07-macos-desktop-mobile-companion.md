# ADR: macOS Desktop and Mobile Companion Direction

- Status: Accepted
- Date: 2026-10-07

## Context

Yard currently runs as a loopback-only Rust service with a React client. The service owns durable projects, worker profiles, assignments, artifacts, command receipts, policy, reconciliation, and the Herdr adapter. The browser owns presentation and uses xterm.js for terminal rendering. This boundary was deliberately chosen to permit later desktop packaging without moving product logic into a client.

The next product step is a native-feeling macOS application followed by a phone companion. The phone is primarily an operational inbox, not a second full desktop or an independent control plane. Remote connectivity is the highest-risk part: Yard currently has no network authentication, remote authorization, TLS identity, paired-device model, or authenticated event feed. The server rejects non-loopback binds, and interactive terminal WebSockets reject non-loopback origins. Those safeguards must not be relaxed merely to make a phone connect.

## Decision

### Desktop

Build the first desktop application for macOS with Tauri, reusing the existing React client, xterm.js terminal, and Rust control plane.

The desktop shell will:

- launch or attach to one managed `yard-server` instance;
- keep the service and Herdr-backed work running when the last window closes;
- expose explicit **Quit Yard** and **Stop Yard service** operations;
- provide a menu-bar item, actionable-attention badge, and native notifications;
- open one or more Yard windows against the same local service;
- use native file dialogs, deep links, and external-editor integration where useful;
- store device and update credentials in macOS Keychain;
- ship as a signed and notarized universal application when production-ready.

Closing a window is not a workflow-completion signal and must not close Herdr panes, terminate workers, release management leases unrelated to that window, or stop the Yard service. Interactive terminal UI leases held by the closed window may be released normally.

The Tauri process is a client and lifecycle host. Product state and runtime authority remain in `yard-server`. The shell must not write Yard's SQLite database directly or call Herdr as an alternate control path.

### Mobile

Build a mobile companion inbox after the authenticated remote boundary exists. Begin with iPhone/iOS as the product target. Reuse shared TypeScript API contracts and domain presentation logic, but permit a mobile-specific information architecture.

The initial mobile surface includes:

- actionable worker questions and approval requests backed by durable Yard records;
- assignment and worker state changes;
- completion receipts and summary artifacts;
- project and worker status;
- bounded structured prompts and steering;
- notification deep links into the exact durable item;
- offline, reconnecting, stale, revoked, and unsupported states.

It does not initially include:

- the full spatial city;
- interactive terminal input;
- arbitrary shell commands;
- project, worker, worktree, or terminal creation;
- profile, automation, device, or server administration;
- destructive cleanup or runtime closure;
- automatic model/token spending.

Read-only bounded terminal observation may be evaluated after the inbox is working. Interactive terminal access requires a separate decision and acceptance suite.

### Remote authority

A paired phone receives least-privilege scopes. Initial scopes are:

- `projects.read`;
- `workers.read`;
- `inbox.read`;
- `artifacts.read` for explicitly mobile-safe artifact types;
- `prompts.create` for one explicit target;
- `approvals.resolve` for one durable pending request and expected revision.

Every mobile mutation uses Yard's existing command-ID, actor, expected-version, idempotency, and audit semantics. Pairing never grants terminal interaction, administration, destructive lifecycle control, or automatic token spending implicitly. Token-spending prompts remain explicit user actions and must display the target and effect before submission.

The companion must consume durable typed requests and lifecycle events. It must not infer questions, approvals, completion, or messages from terminal output.

### Connectivity sequence

Use two connectivity stages:

1. **Private-network preview.** Support an authenticated connection over an operator-provided private network such as Tailscale or WireGuard. This validates pairing, authorization, inbox behavior, retries, and revocation without operating a public relay. A tunnel is transport, not authentication.
2. **Yard relay.** Add a minimal hosted relay for consumer-grade reachability only after the paired-device protocol is proven. Desktop and phone both make outbound connections. Application payloads are end-to-end encrypted between paired devices; the relay routes opaque bounded envelopes and cannot exercise Yard authority or decrypt project content.

The local browser API remains loopback-only. Remote traffic enters through a distinct authenticated gateway and policy boundary. Do not change `YARD_BIND` to `0.0.0.0` or broaden the existing terminal-origin allowlist as an implementation shortcut.

### Pairing and device identity

Pairing uses a short-lived, single-use QR invitation displayed by the desktop app. The invitation bootstraps mutually authenticated device keys and contains no reusable bearer credential. The desktop shows the requesting device and the exact initial scopes before approval.

Each paired device has:

- a stable public-key identity;
- an operator-visible name and platform;
- granted scopes and policy revision;
- created, last-seen, expiry, and revoked state;
- independently rotatable session keys;
- a complete authorization audit trail.

Private keys live in Keychain on macOS and Keychain/secure hardware-backed storage on iOS where available. Revocation takes effect before queued commands execute. Missing policy, stale revisions, clock uncertainty outside the allowed window, replayed envelopes, and identity mismatch fail closed.

### Protocol foundations

Before mobile product work, Yard will add:

- generated or mechanically checked Rust/TypeScript wire contracts;
- explicit protocol and client-capability negotiation;
- authenticated actors and project-scoped authorization;
- durable inbox/request records for questions and approvals;
- a cursor-based, resumable, bounded event feed;
- per-device command replay protection and rate limits;
- snapshot-plus-cursor recovery after disconnection;
- safe notification summaries that contain no terminal bytes, secrets, or arbitrary artifact content;
- device pairing, scope editing, expiry, and revocation UX;
- audit records for actor, device, project, target, action, policy revision, correlation ID, and result.

Terminal frames remain a separate bounded stream and never enter the durable event ledger or mobile push payloads.

## Desktop process model

```text
Yard.app
  Tauri host
    window(s): React + xterm.js
    menu bar / notifications / deep links
    lifecycle supervisor
       |
       | loopback authenticated client session
       v
  yard-server (one owner per database)
    SQLite / artifacts / policy / reconciliation
       |
       v
  Herdr daemon and terminal processes
```

The shell first discovers a compatible managed service for the configured database. If none exists, it starts the bundled service on an ephemeral loopback port and waits for a cryptographically random bootstrap handshake delivered through an owner-only local channel, not command-line arguments or URLs. Attaching to an incompatible or ambiguously owned service fails visibly. Service logs and diagnostics are accessible without exposing secrets.

## Mobile connection model

```text
iPhone companion
  |  paired identity, encrypted envelopes
  |  private network initially / opaque relay later
  v
Yard remote gateway on the Mac
  |  authenticate -> authorize -> validate revision -> audit
  v
Existing Yard application services and command handlers
```

The remote gateway adapts authenticated envelopes to the same application-service methods used locally. It is not a second orchestrator and stores no alternate workflow truth.

## Delivery slices

### Slice 0: contracts and lifecycle design

- Define desktop client configuration and service discovery contracts.
- Define managed-service ownership and window-close/quit behavior.
- Define mobile inbox, device, scope, request, and event-cursor types.
- Threat-model pairing, remote commands, notifications, relay metadata, and revocation.
- Decide the generated-schema mechanism before adding a second independently released client.

Exit: accepted contracts and threat model, with no remote listener exposed.

### Slice 1: macOS Tauri proof of concept

- Add a Tauri workspace outside `web` while reusing `web/dist`.
- Launch or attach to `yard-server` on a dynamic loopback port.
- Load health, map, Files/Review, and one interactive terminal.
- Keep service/workers alive after the window closes.
- Implement explicit quit/stop behavior and diagnostic logs.
- Produce a local unsigned development `.app`.

Exit: one Mac can install and operate Yard without manually starting a browser or server.

### Slice 2: production macOS shell

- Menu bar, dock badge, native notifications, deep links, file dialogs, and external editor actions.
- Single-instance behavior and multiple windows.
- Signed updates with rollback-safe service compatibility.
- Hardened runtime, entitlements, universal build, signing, notarization, and release CI.
- Crash/restart and upgrade tests with active Herdr workers.

Exit: signed/notarized macOS beta with documented recovery.

### Slice 3: durable companion inbox

- Persist typed questions/approval requests instead of browser-only terminal inference.
- Add cursor-based lifecycle/inbox feed and snapshot recovery.
- Build desktop inbox and device-management UX first.
- Add authorization and audit checks at application-service boundaries.

Exit: the local desktop can exercise the exact API and policy the phone will use.

### Slice 4: private-network iPhone companion

- Pair through a one-time QR invitation.
- Connect over an operator-provided private network.
- Implement inbox, status, artifacts, explicit prompts, and approval resolution.
- Store keys securely and support revoke, expiry, reconnect, stale state, and offline read cache.
- Keep push notifications out of scope unless a trusted delivery path is available; use foreground/live updates initially.

Exit: paired iPhone companion works without making Yard publicly reachable.

### Slice 5: encrypted relay and notifications

- Operate an authenticated, rate-limited relay carrying opaque encrypted envelopes.
- Add key rotation, multi-device delivery, abuse controls, retention limits, and relay observability without payload access.
- Add APNs with content-minimal wake notifications; fetch protected content after device authentication.
- Document metadata visibility, availability expectations, account recovery, and relay shutdown behavior.

Exit: the companion works away from the private network without giving the relay Yard authority or plaintext project data.

## Acceptance invariants

- One database has one active Yard service owner.
- Closing every window leaves the service and workers running by default.
- Quit and Stop Service are distinct, explicit operations.
- A shell crash cannot complete assignments or close runtime panes.
- Desktop packaging does not create a second database or Herdr control path.
- Existing browser mode remains supported during migration.
- A phone cannot connect before explicit pairing and desktop approval.
- Every remote mutation names one durable target, actor, command ID, expected revision, and policy revision.
- Revoked or expired devices cannot read new events or execute queued commands.
- Replayed commands are harmless; changed payloads under a reused ID are rejected.
- Notification services and the future relay never receive terminal output or plaintext artifact content.
- The phone never treats runtime status as workflow completion.
- Automatic token spending remains disabled unless separately enabled, and phone-originated spend is always an explicit action.
- No implementation requires a non-loopback bind on the existing local API.

## Consequences

Tauri provides the shortest path to a native macOS product while preserving Yard's tested React UI and Rust control plane. The companion is intentionally narrower than desktop and therefore can optimize for timely decisions rather than reproducing the city and terminal workspace.

Connectivity remains substantial work. The private-network stage provides a usable path without prematurely operating security-critical public infrastructure. The later relay improves reachability while keeping authority and plaintext at the paired endpoints.

The existing proposed Slack transport remains compatible with this decision: Slack, if pursued, is another restricted adapter over the same authenticated command and durable inbox APIs, not the protocol foundation for the Yard mobile app.

## Deferred decisions

- Whether the production iOS shell uses Tauri Mobile or Capacitor will be decided after the shared protocol and mobile UX are exercised as a responsive development client. Framework choice must follow background networking, Keychain, APNs, accessibility, and App Store validation.
- Interactive mobile terminal access.
- Android support.
- Team/multi-user authorization beyond one Yard owner and paired personal devices.
- Relay account recovery and any cross-device recovery service.
- Public plugin access to the remote gateway.

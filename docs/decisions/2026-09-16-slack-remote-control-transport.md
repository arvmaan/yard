# ADR: Slack as a remote-control transport

- Status: Proposed
- Date: 2026-09-16

## Context

Yard currently trusts one local operator. The HTTP server has no user
authentication, project authorization, TLS termination, or remote deployment
model, and rejects non-loopback binds. Terminal WebSockets validate their
browser origin, but the REST router has no equivalent remote-user boundary.

Assignments already accept durable prompt commands. A prompt carries a command
ID, actor, target attempt, expected versions, and text. Replaying the same
command is safe; reusing its ID with different input is a conflict. Yard does
not yet expose a durable structured approval resource or an authenticated event
stream: current agent questions are inferred from terminal output and held in
browser memory.

Slack is useful as a phone-friendly transport, but it must not become another
orchestrator or source of assignment state.

## Decision

Build a thin Slack adapter only after Yard has an authenticated control-plane
API. Slack bot DMs, app mentions, and interactive buttons translate authorized
user intent into that same API. Phone and Mac clients use the same API later.

Slack owns no orchestration state. Yard remains authoritative for projects,
assignments, attempts, prompts, approvals, and lifecycle events. The adapter
stores only delivery and correlation data needed to make transport retries safe:

- Slack workspace, channel, thread, and message IDs mapped to a Yard project,
  assignment, attempt, and last delivered Yard revision;
- processed Slack event IDs, verified interaction digests, and deterministic Yard
  command IDs;
- queued outbound updates and their delivery result.

Do not add a Slack SDK or implementation scaffold in this slice. The current
missing auth and approval boundaries make even a small inbound endpoint unsafe.

## Inbound boundary

The adapter must verify Slack's signature over the exact raw request body before
JSON or form decoding. It uses the configured signing secret, Slack's versioned
base string, constant-time comparison, and rejects timestamps outside a
five-minute window. Verification failure is a generic unauthorized response and
is never retried internally.

Slack retries are normal. For Events API callbacks, the adapter records the
Slack `event_id` and derives the Yard prompt command ID from it. For
`block_actions`, it records a digest of a canonical serialization of the
verified decoded interaction payload and derives the Yard approval command ID
from that digest; key ordering or form encoding cannot change idempotency.
Repeats are successful replays. The adapter acknowledges Slack within three
seconds and completes Yard work from a durable local inbox.

The replay-key insert and inbox enqueue are one transaction; acknowledgement
occurs only after that commit. The inbox retains the normalized command and
correlation data, not the raw signed body beyond the replay window.

Only these verified inbound shapes may proceed to normalization. Required fields
are evaluated by payload type, not as one shared Slack schema:

- An Events API `event_callback` must contain outer `team_id`, `api_app_id`,
  and `event_id`, plus a human-authored inner event with `user`, `channel`,
  `ts`, and non-empty `text`. The inner event is either `message` with
  `channel_type == "im"`, or `app_mention` in an explicitly allowed internal
  channel. Its `subtype` must be absent or null.
- An interactive `block_actions` payload must contain `team.id`, `api_app_id`,
  `user.id`, a message `container` with `channel_id` and `message_ts`, and
  exactly one action with a recognized Approve or Reject `action_id`,
  `action_ts`, and opaque value carrying the Yard request ID and revision.
  It does not require Events API `text` or `event_id` fields.

`url_verification` is handled only during app setup and never becomes a Yard
command. Socket Mode accepts the same inner event and interaction allowlist
after authenticated envelope validation; it does not broaden accepted events.

After transport authentication and minimal top-level decoding, the adapter
branches on payload type. Unsupported payload types are rejected before common
command extraction, policy lookup, target resolution, replay-key insertion,
inbox enqueue, or any Yard API call. Events API callbacks are rejected when
required event fields are absent, or for `bot_message`, `message_changed`,
`message_deleted`, hidden or edited messages, Slack system events, `bot_id` or
`bot_profile`, the adapter's own bot user, or any unsupported event type or
subtype. `block_actions` payloads are rejected when required interaction,
action, container, or human-user metadata is absent or unsupported; Events API
`text` and `event_id` requirements are never applied to them. Rejections retain
only a bounded reason and correlation ID.

After signature verification, authorization is independent of Slack channel
membership. An explicit allowlist maps Slack workspace and user IDs to Yard
project IDs and permitted actions. Every request resolves one concrete target;
ambiguous project or assignment selection is rejected. Authorization is checked
again when the queued command runs, not only when it enters the inbox.

The authorization policy is operator-owned control-plane configuration. It is
revisioned and audited, and each accepted command records the policy revision
used for its decision. Project workers cannot read or modify it: it is not
stored in a project worktree, exposed through worker-facing APIs, injected into
worker processes, or writable with project-worker credentials. Missing or
unavailable policy fails closed.

DMs require an allowed user. App mentions additionally require an allowed
channel ID and type; shared or external channels are rejected in the first
slice.

Interactive buttons must include an opaque, single-use Yard approval/request ID
and expected revision. They must never carry prompt text, shell commands, paths,
or authority in Slack-controlled fields. Stale, already-resolved, or
unauthorized actions return a harmless status update.

HTTP callbacks use signing-secret verification. Socket Mode instead uses an
authenticated outbound WebSocket, deduplicates Slack envelope IDs, and
acknowledges each envelope; it does not reuse the HTTP signature path.

## Outbound behavior

A DM or mention starts one Slack thread for one Yard target. The adapter posts a
short accepted/rejected acknowledgement, then updates that thread from Yard
lifecycle events. Message updates are revision-aware and idempotent. Raw
terminal output is not mirrored by default; summaries must be bounded and
secrets-safe. A transactional outbox survives restart, respects the Slack
Retry-After header, escapes user-controlled mentions, and disables unfurls
unless explicitly needed.

The adapter may poll a narrowly scoped status endpoint in the first slice. It
must not scrape terminal text for approval decisions. A durable approval API is
required before approval buttons ship, and an authenticated event cursor is the
preferred follow-up for efficient status delivery.

## Secrets and deployment

Slack signing secrets and bot tokens live in an OS secret store or an
owner-readable configuration source, never Yard's project database, browser
storage, logs, screenshots, or command payloads. Rotation must allow replacing
each secret independently with bounded overlap.

The first slice is a single-workspace internal app using no user tokens. Its bot
token is limited to app_mentions:read, im:history, and chat:write. Socket Mode
additionally uses an app-level token limited to connections:write. Uninstall or
token revocation disables delivery until credentials are replaced.

Yard remains loopback-only. A public Slack callback cannot be pointed directly
at the current server. Deployment must use either a separately hardened HTTPS
adapter that calls Yard over an authenticated loopback API, or Slack Socket Mode
with the same Yard-side authentication and authorization. A tunnel alone is not
a security boundary.

Before any remote exposure, Yard must have:

- authenticated callers and project-scoped authorization on control APIs;
- a durable approval/request resource with revision and audit history;
- an authenticated, cursor-based lifecycle/status feed or bounded polling API;
- TLS or an authenticated outbound connection, request size/rate limits, and
  safe secret storage;
- audit records for actor, project, target, action, result, and correlation ID;
- an operator-visible revoke/disable path and documented recovery behavior.

## Smallest usable vertical slice

1. Configure one Slack workspace, allowed Slack users and channels, and one Yard
   project.
2. Accept bot DMs and app mentions naming one existing active assignment.
3. Verify, authorize, deduplicate, enqueue, and submit message text through the
   existing Yard prompt command API.
4. Reply in one Slack thread with accepted, rejected, running, and terminal
   completion/failure status.
5. Once the durable approval API exists, add one Approve/Reject interaction for
   that exact pending request using expected-revision concurrency.

## Non-goals

- Project, assignment, agent, worktree, or terminal creation from Slack.
- General terminal streaming, arbitrary shell execution, or file transfer.
- Slack-owned queues, retries, workflow state, policy, or approval history.
- Natural-language target guessing across projects.
- Multi-workspace administration, marketplace distribution, or a Slack SDK.
- A separate phone/Mac orchestration protocol.

## Consequences

The first remote slice stays small and uses Yard's durable command semantics.
Delivery correlation is local to the adapter and can be rebuilt without
changing Yard orchestration state. Remote use is intentionally blocked until
the missing authenticated control-plane and approval boundaries exist.

## References

- [Slack request verification](https://docs.slack.dev/authentication/verifying-requests-from-slack/)
- [Slack Events API](https://docs.slack.dev/apis/events-api/)
- [Slack interactivity handling](https://docs.slack.dev/interactivity/handling-user-interaction/)

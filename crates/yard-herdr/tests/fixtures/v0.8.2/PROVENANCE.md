# Herdr Fixture Provenance

These fixtures are synthetic and contain no user runtime data. Their shape was
checked against the Herdr `v0.8.2` tag at commit `9eb5214` (protocol 20) on
2026-09-04.

Primary source references:

- `src/protocol/wire.rs` (`PROTOCOL_VERSION`)
- `src/session.rs` (`SessionInfo`)
- `src/app/api/session.rs` (`SessionSnapshot` assembly)
- `src/api/schema/session.rs`
- `src/api/schema/workspaces.rs`
- `src/api/schema/tabs.rs`
- `src/api/schema/panes.rs`
- `src/api/schema/agents.rs`
- `src/api/schema/response.rs`
- `src/app/api/workspaces.rs` (`workspace.close`)
- `src/app/api/tabs.rs` (`tab.close`)

The snapshot includes the v0.8.2 layout, title, state-label, scroll, and
screen-detection fields that Yard does not consume. This verifies that Yard
accepts the complete protocol-20 shape while normalizing only its supported
inventory fields.

`control.json` follows the v0.8.2 `ResponseResult` serialization and its
`skip_serializing_if` omissions for empty, false, and absent fields.

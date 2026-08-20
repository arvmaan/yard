# Contributing to Yard

## Change Discipline

Keep each change tied to one durable objective. Preserve the ownership
boundary: Herdr owns live runtime resources; Yard owns durable control intent.
Do not infer completion from runtime status or prompt delivery.

Comment non-obvious invariants, ambiguity decisions, durability boundaries,
and browser or runtime constraints. Avoid comments that restate the code.

## Setup

```sh
cargo fetch
cd web
npm ci
npx playwright install chromium
```

Use feature branches. Do not include local Yard data, credentials, terminal
transcripts, or provider state.

## Required Gates

From the repository root:

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
bash -n scripts/*.sh
git diff --check
```

From `web/`:

```sh
npm ci
npm run lint
npm run build
npm run test:unit
npm run test:e2e
```

Run `scripts/embedded-binary-smoke.sh` and
`scripts/cli-lifecycle-smoke.sh` when changing executable packaging, startup,
shutdown, or configuration.

Run `./scripts/live-herdr-smoke.sh` when changing the Herdr adapter, runtime
creation, reconciliation, or terminal transport. The one-hour historical
acceptance harness is an explicit release gate, not a routine test; it uses
authenticated unrestricted agents.

## Review Evidence

A review should state:

- behavior changed and invariants preserved;
- exact commands and results;
- manual UI checks performed;
- known risks and deferred work;
- whether a manual Yard receipt was issued by the owner.

Do not create a receipt, release tag, or owner approval on someone else's
behalf.

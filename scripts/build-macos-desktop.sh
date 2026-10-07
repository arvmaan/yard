#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
MODE=${1:-build}
TARGET_TRIPLE=$(rustc -vV | sed -n 's/^host: //p')
SIDECAR="$ROOT/crates/yard-desktop/binaries/yard-$TARGET_TRIPLE"

if [[ "$(uname -s)" != Darwin ]]; then
  printf 'This script builds/runs the unsigned macOS Yard.app POC and must run on macOS.\n' >&2
  exit 2
fi
if [[ "$MODE" != build && "$MODE" != dev ]]; then
  printf 'usage: %s [build|dev]\n' "$0" >&2
  exit 2
fi
if ! command -v cargo-tauri >/dev/null 2>&1; then
  printf 'cargo-tauri 2.9.6 is required; install it with:\n' >&2
  printf '  cargo install tauri-cli --version 2.9.6 --locked\n' >&2
  exit 2
fi

cleanup() {
  rm -f "$SIDECAR"
}
trap cleanup EXIT

cd "$ROOT/web"
npm ci
npm run build

cd "$ROOT"
cargo build --release --locked -p yard-server --bin yard
mkdir -p "$(dirname "$SIDECAR")"
cp target/release/yard "$SIDECAR"

cd "$ROOT/crates/yard-desktop"
if [[ "$MODE" == dev ]]; then
  cargo tauri dev
else
  cargo tauri build --bundles app
  printf 'Unsigned development app: %s\n' \
    "$ROOT/target/release/bundle/macos/Yard.app"
fi

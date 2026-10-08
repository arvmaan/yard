#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
MODE=${1:-build}
TARGET_TRIPLE=$(rustc -vV | sed -n 's/^host: //p')
SIDECAR_DIRECTORY="$ROOT/crates/yard-desktop/binaries"
YARD_SIDECAR="$SIDECAR_DIRECTORY/yard-$TARGET_TRIPLE"
HERDR_SIDECAR="$SIDECAR_DIRECTORY/herdr-$TARGET_TRIPLE"
MINIMUM_HERDR_VERSION=0.9.3

if [[ "$(uname -s)" != Darwin ]]; then
  printf 'This script builds/runs the unsigned macOS Yard.app POC and must run on macOS.\n' >&2
  exit 2
fi
if [[ "$TARGET_TRIPLE" != *-apple-darwin ]]; then
  printf 'Rust host target %s is not a macOS target.\n' "$TARGET_TRIPLE" >&2
  exit 2
fi
if ! command -v node >/dev/null 2>&1 || ! node -e '
const [major, minor] = process.versions.node.split(".").map(Number)
process.exit(major > 22 || (major === 22 && minor >= 12) || (major === 20 && minor >= 19) ? 0 : 1)
'; then
  printf 'Yard requires Node.js 20.19+ or 22.12+; the recommended version is 22.12.0.\n' >&2
  printf 'Install/use it with your Node manager, for example: nvm install 22.12.0 && nvm use 22.12.0\n' >&2
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

HERDR_BIN=${YARD_DESKTOP_BUILD_HERDR_BIN:-}
if [[ -z "$HERDR_BIN" ]]; then
  if ! HERDR_BIN=$(command -v herdr) || [[ -z "$HERDR_BIN" ]]; then
    printf 'Herdr %s+ is required. Install Herdr or set YARD_DESKTOP_BUILD_HERDR_BIN to its absolute executable path.\n' \
      "$MINIMUM_HERDR_VERSION" >&2
    exit 2
  fi
fi
if [[ "$HERDR_BIN" != /* || ! -f "$HERDR_BIN" || ! -x "$HERDR_BIN" ]]; then
  printf 'Herdr build input must be an absolute executable file: %s\n' "$HERDR_BIN" >&2
  exit 2
fi
HERDR_BIN=$(cd "$(dirname "$HERDR_BIN")" && pwd -P)/$(basename "$HERDR_BIN")

HERDR_VERSION_OUTPUT=$("$HERDR_BIN" --version 2>&1) || {
  printf 'Could not execute Herdr build input %s.\n' "$HERDR_BIN" >&2
  exit 2
}
if [[ "$HERDR_VERSION_OUTPUT" =~ ^herdr[[:space:]]+([0-9]+\.[0-9]+\.[0-9]+)(\+[^[:space:]]+)?$ ]]; then
  HERDR_VERSION=${BASH_REMATCH[1]}
else
  printf 'Build input is not a recognizable Herdr CLI: %s reported %s\n' \
    "$HERDR_BIN" "$HERDR_VERSION_OUTPUT" >&2
  exit 2
fi
version_at_least() {
  local candidate=$1 minimum=$2
  local candidate_major candidate_minor candidate_patch
  local minimum_major minimum_minor minimum_patch
  IFS=. read -r candidate_major candidate_minor candidate_patch <<<"$candidate"
  IFS=. read -r minimum_major minimum_minor minimum_patch <<<"$minimum"
  ((candidate_major > minimum_major)) ||
    ((candidate_major == minimum_major && candidate_minor > minimum_minor)) ||
    ((candidate_major == minimum_major && candidate_minor == minimum_minor && candidate_patch >= minimum_patch))
}
if ! version_at_least "$HERDR_VERSION" "$MINIMUM_HERDR_VERSION"; then
  printf 'Herdr %s does not meet the minimum machine-forwarding version %s.\n' \
    "$HERDR_VERSION" "$MINIMUM_HERDR_VERSION" >&2
  exit 2
fi
HERDR_HELP=$("$HERDR_BIN" --help 2>&1)
MACHINE_LIST_HELP=$("$HERDR_BIN" machine list --help 2>&1)
if [[ "$HERDR_HELP" != *'--machine <label-or-id>'* || "$MACHINE_LIST_HELP" != *'--json'* ]]; then
  printf 'Herdr %s lacks the saved-machine catalog or machine-forwarding CLI required by Yard.\n' \
    "$HERDR_VERSION" >&2
  exit 2
fi

case "$TARGET_TRIPLE" in
  aarch64-apple-darwin) EXPECTED_ARCH=arm64 ;;
  x86_64-apple-darwin) EXPECTED_ARCH=x86_64 ;;
  *)
    printf 'Unsupported macOS target architecture: %s\n' "$TARGET_TRIPLE" >&2
    exit 2
    ;;
esac
HERDR_FILE=$(file -b "$HERDR_BIN")
if [[ "$HERDR_FILE" != *Mach-O* || "$HERDR_FILE" != *"$EXPECTED_ARCH"* ]]; then
  printf 'Herdr build input must be a %s Mach-O binary for %s; got: %s\n' \
    "$EXPECTED_ARCH" "$TARGET_TRIPLE" "$HERDR_FILE" >&2
  exit 2
fi

cleanup() {
  rm -f "$YARD_SIDECAR" "$HERDR_SIDECAR"
}
terminate() {
  local signal=$1
  trap - EXIT HUP INT TERM
  cleanup
  kill -s "$signal" "$$"
}
trap cleanup EXIT
trap 'terminate HUP' HUP
trap 'terminate INT' INT
trap 'terminate TERM' TERM

cd "$ROOT/web"
npm ci
npm run build

cd "$ROOT"
cargo build --release --locked -p yard-server --bin yard
mkdir -p "$SIDECAR_DIRECTORY"
cp target/release/yard "$YARD_SIDECAR"
cp "$HERDR_BIN" "$HERDR_SIDECAR"

cd "$ROOT/crates/yard-desktop"
if [[ "$MODE" == dev ]]; then
  cargo tauri dev
else
  cargo tauri build --bundles app
  printf 'Unsigned development app: %s\n' \
    "$ROOT/target/release/bundle/macos/Yard.app"
  printf 'Bundled sidecars: Contents/MacOS/yard and Contents/MacOS/herdr (Herdr %s, %s)\n' \
    "$HERDR_VERSION" "$TARGET_TRIPLE"
fi

#!/usr/bin/env bash

set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

if (($# > 1)); then
  printf 'Usage: %s [BIN_DIR]\n' "$0" >&2
  exit 2
fi

if (($# == 1)); then
  BIN_DIR=$1
elif [[ -n "${YARD_INSTALL_DIR:-}" ]]; then
  BIN_DIR=$YARD_INSTALL_DIR
elif [[ -n "${HOME:-}" ]]; then
  BIN_DIR=$HOME/.local/bin
else
  printf 'HOME is not set; pass an install directory or set YARD_INSTALL_DIR\n' >&2
  exit 2
fi

cd "$REPO_ROOT"
npm --prefix web ci
cargo build --release --locked -p yard-server --bin yard

TARGET_DIR=${CARGO_TARGET_DIR:-"$REPO_ROOT/target"}
if [[ "$TARGET_DIR" != /* ]]; then
  TARGET_DIR=$REPO_ROOT/$TARGET_DIR
fi
SOURCE=$TARGET_DIR/release/yard
[[ -x "$SOURCE" ]] || {
  printf 'Built Yard executable was not found at %s\n' "$SOURCE" >&2
  exit 1
}

mkdir -p "$BIN_DIR"
BIN_DIR=$(cd "$BIN_DIR" && pwd)
DESTINATION=$BIN_DIR/yard
[[ ! -d "$DESTINATION" ]] || {
  printf 'Cannot install Yard: destination is a directory: %s\n' "$DESTINATION" >&2
  exit 1
}

TEMPORARY=$(mktemp "$BIN_DIR/.yard.install.XXXXXX")
cleanup() {
  rm -f "$TEMPORARY"
}
trap cleanup EXIT INT TERM
install -m 0755 "$SOURCE" "$TEMPORARY"
mv -f "$TEMPORARY" "$DESTINATION"
trap - EXIT INT TERM

printf 'Installed Yard to %s\n' "$DESTINATION"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    printf 'Add %s to PATH, then run: yard start\n' "$BIN_DIR"
    ;;
esac

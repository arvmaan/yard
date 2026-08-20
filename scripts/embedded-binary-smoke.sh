#!/usr/bin/env bash

set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
BINARY=${1:-"$REPO_ROOT/target/release/yard-server"}
if [[ "$BINARY" != /* ]]; then
  BINARY="$(cd "$(dirname "$BINARY")" && pwd)/$(basename "$BINARY")"
fi
[[ -x "$BINARY" ]] || {
  printf 'Yard binary is not executable: %s\n' "$BINARY" >&2
  printf 'Build it with: cargo build --release --locked -p yard-server\n' >&2
  exit 1
}

ROOT=$(mktemp -d "${TMPDIR:-/tmp}/yard-embedded-smoke.XXXXXX")
YARD_PID=''

cleanup() {
  if [[ -n "$YARD_PID" ]] && kill -0 "$YARD_PID" >/dev/null 2>&1; then
    kill "$YARD_PID" >/dev/null 2>&1 || true
    wait "$YARD_PID" 2>/dev/null || true
  fi
  find "$ROOT" -depth -delete
}
trap cleanup EXIT INT TERM

curl_request() {
  curl --connect-timeout 1 --max-time 5 "$@"
}

mkdir -p "$ROOT/run" "$ROOT/data" "$ROOT/empty-path"
cp "$BINARY" "$ROOT/yard-server"
[[ ! -e "$ROOT/run/web" ]]

(
  cd "$ROOT/run"
  PATH="$ROOT/empty-path" \
  YARD_BIND=127.0.0.1:0 \
  YARD_DATABASE_PATH="$ROOT/data/yard.sqlite3" \
  YARD_ARTIFACT_PATH="$ROOT/data/artifacts" \
  YARD_COORDINATION_PATH="$ROOT/data/coordination" \
  YARD_KNOWLEDGE_PATH="$ROOT/data/knowledge" \
  YARD_ORCHESTRATOR_CWD="$ROOT/run" \
  YARD_HERDR_BIN="$ROOT/missing-herdr" \
  RUST_LOG=yard_server=info \
    "$ROOT/yard-server" >"$ROOT/yard.log" 2>&1
) &
YARD_PID=$!

BASE_URL=''
for _ in {1..200}; do
  if ! kill -0 "$YARD_PID" >/dev/null 2>&1; then
    sed -n '1,160p' "$ROOT/yard.log" >&2
    exit 1
  fi
  BASE_URL=$(sed -n \
    's/.*ui_url=\(http:\/\/127\.0\.0\.1:[0-9][0-9]*\/\).*/\1/p' \
    "$ROOT/yard.log" | tail -n 1)
  [[ -n "$BASE_URL" ]] && break
  sleep 0.05
done
[[ -n "$BASE_URL" ]] || {
  printf 'Yard did not print its UI URL\n' >&2
  exit 1
}

for _ in {1..200}; do
  if curl_request -fsS "${BASE_URL}health" >"$ROOT/health.body" 2>/dev/null; then
    break
  fi
  sleep 0.05
done

HEALTH_CODE=$(curl_request -sS -o "$ROOT/health.body" -w '%{http_code}' \
  "${BASE_URL}health")
UI_CODE=$(curl_request -sS -D "$ROOT/ui.headers" -o "$ROOT/ui.body" \
  -w '%{http_code}' "$BASE_URL")
ASSET_PATH=$(sed -n 's/.*src="\(\/assets\/[^"]*\.js\)".*/\1/p' \
  "$ROOT/ui.body" | head -n 1)
[[ -n "$ASSET_PATH" ]] || {
  printf 'Embedded index did not reference a JavaScript asset\n' >&2
  exit 1
}
ASSET_CODE=$(curl_request -sS -D "$ROOT/asset.headers" -o "$ROOT/asset.body" \
  -w '%{http_code}' "${BASE_URL%/}${ASSET_PATH}")
SPA_CODE=$(curl_request -sS -D "$ROOT/spa.headers" -o "$ROOT/spa.body" \
  -w '%{http_code}' "${BASE_URL}projects/smoke/overview")
API_CODE=$(curl_request -sS -D "$ROOT/api.headers" -o "$ROOT/api.body" \
  -w '%{http_code}' "${BASE_URL}api/v1/projects")
UNKNOWN_API_CODE=$(curl_request -sS -D "$ROOT/unknown-api.headers" \
  -o "$ROOT/unknown-api.body" -w '%{http_code}' \
  "${BASE_URL}api/v1/not-a-route")

[[ "$HEALTH_CODE" == 200 ]]
[[ "$UI_CODE" == 200 ]]
[[ "$ASSET_CODE" == 200 ]]
[[ "$SPA_CODE" == 200 ]]
[[ "$API_CODE" == 200 ]]
[[ "$UNKNOWN_API_CODE" == 404 ]]
grep -q '"status":"ok"' "$ROOT/health.body"
grep -q '<div id="root"></div>' "$ROOT/ui.body"
cmp -s "$ROOT/ui.body" "$ROOT/spa.body"
grep -qi '^content-type: text/javascript; charset=utf-8' "$ROOT/asset.headers"
grep -qi '^cache-control: public, max-age=31536000, immutable' \
  "$ROOT/asset.headers"
grep -qi '^cache-control: no-cache' "$ROOT/ui.headers"
grep -qi '^cache-control: no-cache' "$ROOT/spa.headers"
grep -qi '^content-type: application/json' "$ROOT/api.headers"
grep -q '"projects":\[\]' "$ROOT/api.body"
! grep -qi '^content-type: text/html' "$ROOT/unknown-api.headers"

printf 'Yard embedded-binary smoke passed\n'
printf '  UI: %s (%s)\n' "$BASE_URL" "$UI_CODE"
printf '  Health: %s, asset: %s, SPA: %s, API: %s, unknown API: %s\n' \
  "$HEALTH_CODE" "$ASSET_CODE" "$SPA_CODE" "$API_CODE" "$UNKNOWN_API_CODE"
printf '  Asset: %s (%s bytes)\n' "$ASSET_PATH" "$(wc -c <"$ROOT/asset.body")"

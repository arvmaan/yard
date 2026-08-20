#!/usr/bin/env bash

set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ROOT=$(mktemp -d /tmp/yard-herdr-e2e.XXXXXX)
SESSION="yard-e2e-$$"
PORT=${YARD_HERDR_SMOKE_PORT:-}
export XDG_CONFIG_HOME="$ROOT/config"
export XDG_STATE_HOME="$ROOT/state"
export XDG_DATA_HOME="$ROOT/data"
export XDG_CACHE_HOME="$ROOT/cache"
export XDG_RUNTIME_DIR="$ROOT/runtime"

HERDR_PID=''
YARD_PID=''
WORKSPACE_ID=''

cleanup() {
  if [[ -n "$WORKSPACE_ID" ]]; then
    herdr --session "$SESSION" workspace close "$WORKSPACE_ID" >/dev/null 2>&1 || true
  fi
  herdr session stop "$SESSION" --json >/dev/null 2>&1 || true
  if [[ -n "$YARD_PID" ]]; then
    kill "$YARD_PID" >/dev/null 2>&1 || true
    wait "$YARD_PID" 2>/dev/null || true
  fi
  if [[ -n "$HERDR_PID" ]]; then
    kill "$HERDR_PID" >/dev/null 2>&1 || true
    wait "$HERDR_PID" 2>/dev/null || true
  fi
  herdr session delete "$SESSION" --json >/dev/null 2>&1 || true
  find "$ROOT" -depth -delete
}
trap cleanup EXIT INT TERM

cd "$REPO_ROOT"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
cargo build -p yard-server >/dev/null

if [[ -z "$PORT" ]]; then
  PORT=$(node -e '
    const server = require("node:net").createServer();
    server.listen(0, "127.0.0.1", () => {
      process.stdout.write(String(server.address().port));
      server.close();
    });
  ')
fi

HERDR_STARTUP_CWD=/tmp herdr --session "$SESSION" server >"$ROOT/herdr.log" 2>&1 &
HERDR_PID=$!
for _ in {1..200}; do
  if herdr --session "$SESSION" status server --json >/dev/null 2>&1; then
    break
  fi
  sleep 0.05
done
herdr --session "$SESSION" status server --json >/dev/null
BASELINE_WORKSPACE_COUNT=$(herdr --session "$SESSION" workspace list |
  jq '.result.workspaces | length')

YARD_BIND="127.0.0.1:$PORT" \
YARD_DATABASE_PATH="$ROOT/yard.sqlite3" \
YARD_ARTIFACT_PATH="$ROOT/artifacts" \
RUST_LOG=yard_server=debug \
./target/debug/yard run >"$ROOT/yard.log" 2>&1 &
YARD_PID=$!
for _ in {1..200}; do
  if ! kill -0 "$YARD_PID" >/dev/null 2>&1; then
    tail -n 80 "$ROOT/yard.log" >&2
    exit 1
  fi
  if curl -fsS "http://127.0.0.1:$PORT/health" >/dev/null 2>&1; then
    break
  fi
  sleep 0.05
done
sleep 0.05
kill -0 "$YARD_PID" >/dev/null 2>&1 || {
  tail -n 80 "$ROOT/yard.log" >&2
  exit 1
}
curl -fsS "http://127.0.0.1:$PORT/health" >/dev/null

PROFILE_BODY=$(jq -nc '{
  name: "Live orchestrator",
  runtime_adapter: "herdr",
  provider: "codex",
  model: null,
  default_role: "orchestrator",
  instructions_ref: null,
  tools: [],
  skills: [],
  mcp_servers: [],
  sandbox_policy: "runtime_default",
  worktree_policy: "project_workspace",
  permission_policy: "yolo",
  completion_contract: "manual_receipt"
}')
PROFILE=$(curl -fsS \
  -X POST "http://127.0.0.1:$PORT/api/v1/worker-profiles" \
  -H 'content-type: application/json' \
  --data "$PROFILE_BODY")
PROFILE_ID=$(jq -er '.id' <<<"$PROFILE")
COMMAND_ID=$(</proc/sys/kernel/random/uuid)
BOOTSTRAP_BODY=$(jq -nc \
  --arg command_id "$COMMAND_ID" \
  --arg session "$SESSION" \
  --arg profile_id "$PROFILE_ID" \
  --arg cwd "$REPO_ROOT" \
  '{
    command_id: $command_id,
    actor: "live-e2e",
    name: "Yard live E2E",
    runtime_adapter: "herdr",
    runtime_session: $session,
    workspace_label: "Yard live E2E",
    cwd: $cwd,
    profile_id: $profile_id,
    expected_profile_version: "1",
    orchestrator_objective: "Reply with READY, then wait. Do not edit files.",
    placement: {x: 80, y: 70, width: 322, height: 240}
  }')

START_MS=$(date +%s%3N)
HTTP_CODE=$(curl -sS \
  --max-time 60 \
  -o "$ROOT/bootstrap.json" \
  -w '%{http_code}' \
  -X POST "http://127.0.0.1:$PORT/api/v1/projects/from-profile/workspace" \
  -H 'content-type: application/json' \
  --data "$BOOTSTRAP_BODY")
END_MS=$(date +%s%3N)
if [[ "$HTTP_CODE" != 201 ]]; then
  jq . "$ROOT/bootstrap.json" >&2 || true
  tail -n 80 "$ROOT/yard.log" >&2
  tail -n 80 "$ROOT/herdr.log" >&2
  exit 1
fi

PROJECT=$(<"$ROOT/bootstrap.json")
PROJECT_ID=$(jq -er '.project.id' <<<"$PROJECT")
WORKSPACE_ID=$(jq -er '.project.runtime.workspace_id' <<<"$PROJECT")
TERMINAL_ID=$(jq -er '.project.orchestrator.runtime.terminal_id' <<<"$PROJECT")
PANE_ID=$(jq -er '.project.orchestrator.runtime.pane_id' <<<"$PROJECT")
herdr --session "$SESSION" workspace get "$WORKSPACE_ID" >/dev/null

READY_SEEN=false
for _ in {1..120}; do
  OUTPUT=$(herdr --session "$SESSION" agent read \
    "$PANE_ID" \
    --source recent-unwrapped \
    --lines 80 \
    --format text)
  if rg -q 'READY' <<<"$OUTPUT"; then
    READY_SEEN=true
    break
  fi
  sleep 0.25
done

AGENTS=$(herdr --session "$SESSION" agent list)
REPLAY_CODE=$(curl -sS \
  --max-time 10 \
  -o "$ROOT/replay.json" \
  -w '%{http_code}' \
  -X POST "http://127.0.0.1:$PORT/api/v1/projects/from-profile/workspace" \
  -H 'content-type: application/json' \
  --data "$BOOTSTRAP_BODY")
PROJECT_COUNT=$(curl -fsS "http://127.0.0.1:$PORT/api/v1/projects" |
  jq '.projects | length')
WORKSPACE_COUNT=$(herdr --session "$SESSION" workspace list |
  jq '.result.workspaces | length')
EXPECTED_WORKSPACE_COUNT=$((BASELINE_WORKSPACE_COUNT + 1))
AGENT_COUNT=$(jq '.result.agents | length' <<<"$AGENTS")

jq -n \
  --arg session "$SESSION" \
  --arg project_id "$PROJECT_ID" \
  --arg workspace_id "$WORKSPACE_ID" \
  --arg terminal_id "$TERMINAL_ID" \
  --arg pane_id "$PANE_ID" \
  --arg http_code "$HTTP_CODE" \
  --arg replay_code "$REPLAY_CODE" \
  --argjson elapsed_ms "$((END_MS - START_MS))" \
  --argjson project_count "$PROJECT_COUNT" \
  --argjson baseline_workspace_count "$BASELINE_WORKSPACE_COUNT" \
  --argjson workspace_count "$WORKSPACE_COUNT" \
  --argjson agent_count "$AGENT_COUNT" \
  --argjson replayed "$(jq '.replayed' "$ROOT/replay.json")" \
  --argjson ready_seen "$READY_SEEN" \
  '{
    session: $session,
    http_code: $http_code,
    replay_code: $replay_code,
    elapsed_ms: $elapsed_ms,
    project_id: $project_id,
    workspace_id: $workspace_id,
    terminal_id: $terminal_id,
    pane_id: $pane_id,
    project_count: $project_count,
    baseline_workspace_count: $baseline_workspace_count,
    workspace_count: $workspace_count,
    agent_count: $agent_count,
    replayed: $replayed,
    ready_seen: $ready_seen
  }'

[[ "$REPLAY_CODE" == 200 ]]
[[ "$PROJECT_COUNT" == 1 ]]
[[ "$WORKSPACE_COUNT" == "$EXPECTED_WORKSPACE_COUNT" ]]
[[ "$AGENT_COUNT" == 1 ]]
[[ "$READY_SEEN" == true ]]

#!/usr/bin/env bash

set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
DURATION_SECONDS=${YARD_V1_ACCEPTANCE_DURATION_SECONDS:-3600}
POLL_SECONDS=5
ROOT=$(mktemp -d "${TMPDIR:-/tmp}/yard-v1-acceptance.XXXXXX")
RUN_ID=$(tr -d '-' </proc/sys/kernel/random/uuid | cut -c1-12)
SESSION="yard-v1-${RUN_ID}"
PORT=${YARD_V1_ACCEPTANCE_PORT:-}
WS_MODULE="$REPO_ROOT/web/node_modules/ws"
API_BASE=''
HERDR_PID=''
YARD_PID=''
SOAK_POLLS=0
WORKSPACE_IDS=()

export XDG_CONFIG_HOME="$ROOT/xdg/config"
export XDG_STATE_HOME="$ROOT/xdg/state"
export XDG_DATA_HOME="$ROOT/xdg/data"
export XDG_CACHE_HOME="$ROOT/xdg/cache"
export XDG_RUNTIME_DIR="$ROOT/xdg/runtime"

note() {
  printf 'yard-v1-acceptance: %s\n' "$*" >&2
}

fail() {
  printf 'yard-v1-acceptance: FAIL: %s\n' "$*" >&2
  if [[ -f "$ROOT/yard.log" ]]; then
    printf '%s\n' '--- yard.log (tail) ---' >&2
    tail -n 100 "$ROOT/yard.log" >&2 || true
  fi
  if [[ -f "$ROOT/herdr.log" ]]; then
    printf '%s\n' '--- herdr.log (tail) ---' >&2
    tail -n 100 "$ROOT/herdr.log" >&2 || true
  fi
  local binding_debug
  for binding_debug in "$ROOT"/binding-debug-*.json; do
    [[ -f "$binding_debug" ]] || continue
    printf '%s\n' "--- $(basename "$binding_debug") ---" >&2
    jq . "$binding_debug" >&2 || true
  done
  exit 1
}

herdr_running() {
  local status
  status=$(herdr --session "$SESSION" status server --json 2>/dev/null || true)
  jq -e '.running == true and .status == "running"' >/dev/null 2>&1 <<<"$status"
}

cleanup() {
  local status=$?
  trap - EXIT INT TERM
  set +e

  if [[ -n "$YARD_PID" ]]; then
    kill "$YARD_PID" >/dev/null 2>&1
    wait "$YARD_PID" 2>/dev/null
  fi
  if herdr_running; then
    local workspace_id
    for workspace_id in ${WORKSPACE_IDS[@]+"${WORKSPACE_IDS[@]}"}; do
      herdr --session "$SESSION" workspace close "$workspace_id" >/dev/null 2>&1
    done
  fi
  herdr session stop "$SESSION" --json >/dev/null 2>&1
  if [[ -n "$HERDR_PID" ]]; then
    kill "$HERDR_PID" >/dev/null 2>&1
    wait "$HERDR_PID" 2>/dev/null
  fi
  herdr session delete "$SESSION" --json >/dev/null 2>&1
  find "$ROOT" -depth -delete
  exit "$status"
}
trap cleanup EXIT INT TERM

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "required command is missing: $1"
}

uuid() {
  if [[ -r /proc/sys/kernel/random/uuid ]]; then
    tr '[:upper:]' '[:lower:]' </proc/sys/kernel/random/uuid
  else
    node -e 'console.log(crypto.randomUUID())'
  fi
}

now_ms() {
  date +%s%3N
}

api_json() {
  local method=$1
  local path=$2
  local body=$3
  local expected_codes=$4
  local output=$5
  local code
  local -a args=(
    -sS
    --connect-timeout 3
    --max-time 180
    -o "$output"
    -w '%{http_code}'
    -X "$method"
    "$API_BASE$path"
  )
  if [[ -n "$body" ]]; then
    args+=(-H 'content-type: application/json' --data "$body")
  fi
  if ! code=$(curl "${args[@]}"); then
    fail "$method $path failed before an HTTP response"
  fi
  if [[ " $expected_codes " != *" $code "* ]]; then
    printf 'yard-v1-acceptance: unexpected HTTP %s for %s %s\n' \
      "$code" "$method" "$path" >&2
    jq . "$output" >&2 2>/dev/null || sed -n '1,160p' "$output" >&2
    fail "unexpected Yard API response"
  fi
}

wait_for_yard() {
  local _
  for _ in {1..300}; do
    if curl -fsS --max-time 2 "$API_BASE/health" >/dev/null 2>&1; then
      return
    fi
    if [[ -n "$YARD_PID" ]] && ! kill -0 "$YARD_PID" >/dev/null 2>&1; then
      fail "Yard exited during startup"
    fi
    sleep 0.05
  done
  fail "Yard did not become healthy"
}

start_yard() {
  YARD_BIND="127.0.0.1:$PORT" \
  YARD_DATABASE_PATH="$ROOT/yard.sqlite3" \
  YARD_ARTIFACT_PATH="$ROOT/artifacts" \
  RUST_LOG=yard_server=debug \
  "$REPO_ROOT/target/debug/yard" run >>"$ROOT/yard.log" 2>&1 &
  YARD_PID=$!
  wait_for_yard
}

stop_yard() {
  if [[ -n "$YARD_PID" ]]; then
    kill "$YARD_PID" >/dev/null 2>&1 || true
    wait "$YARD_PID" 2>/dev/null || true
    YARD_PID=''
  fi
}

wait_for_herdr() {
  local _
  for _ in {1..300}; do
    if herdr_running; then
      return
    fi
    if [[ -n "$HERDR_PID" ]] && ! kill -0 "$HERDR_PID" >/dev/null 2>&1; then
      fail "Herdr exited during startup"
    fi
    sleep 0.05
  done
  fail "Herdr did not become healthy"
}

start_herdr() {
  HERDR_STARTUP_CWD="$ROOT/herdr-startup" \
    herdr --session "$SESSION" server >>"$ROOT/herdr.log" 2>&1 &
  HERDR_PID=$!
  wait_for_herdr
}

stop_herdr_process() {
  [[ -n "$HERDR_PID" ]] || fail "Herdr PID is unavailable"
  if ! herdr --session "$SESSION" server stop >"$ROOT/herdr-stop.json"; then
    fail "Herdr rejected its isolated service-stop command"
  fi
  wait "$HERDR_PID" 2>/dev/null || true
  HERDR_PID=''

  local _
  for _ in {1..200}; do
    if ! herdr_running; then
      return
    fi
    sleep 0.05
  done
  fail "Herdr remained reachable after service stop"
}

start_direct_agent() {
  local name=$1
  local pane_id=$2
  local output=$3
  shift 3
  local -a agent_args=(--yolo "$@")
  local _
  for _ in {1..300}; do
    if herdr --session "$SESSION" agent start "$name" \
      --kind codex \
      --pane "$pane_id" \
      --timeout 60000 \
      -- "${agent_args[@]}" >"$output" 2>&1; then
      return
    fi
    if ! jq -e '.error.code == "agent_pane_busy"' "$output" >/dev/null 2>&1; then
      sed -n '1,120p' "$output" >&2 || true
      fail "direct Herdr agent failed to start"
    fi
    sleep 0.1
  done
  fail "direct Herdr pane did not reach an available shell"
}

wait_for_exact_output_line() {
  local path=$1
  local token=$2
  local deadline=$((SECONDS + 180))
  while ((SECONDS < deadline)); do
    api_json GET "$path" '' 200 "$ROOT/output.json"
    if jq -e --arg token "$token" '
      .text
      | split("\n")
      | map(gsub("^[[:space:]]+|[[:space:]]+$"; ""))
      | any(endswith($token) and (contains(":") | not))
    ' "$ROOT/output.json" >/dev/null; then
      return
    fi
    sleep 0.5
  done
  fail "timed out waiting for exact output line: $token"
}

capture_orchestrator_binding() {
  local project_id=$1
  local suffix=$2
  api_json GET "/api/v1/projects/$project_id" '' 200 "$ROOT/binding-project.json"
  api_json GET "/api/v1/runtimes/herdr/sessions/$SESSION/inventory" '' 200 \
    "$ROOT/binding-inventory.json"
  jq -n \
    --slurpfile project "$ROOT/binding-project.json" \
    --slurpfile inventory "$ROOT/binding-inventory.json" '
      ($project[0].orchestrator.runtime) as $persisted
      | ($inventory[0].workers[]
          | select(.terminal_id == $persisted.terminal_id)) as $observed
      | {
          persisted: $persisted,
          observed: {
            workspace_id: $observed.workspace_id,
            terminal_id: $observed.terminal_id,
            tab_id: $observed.tab_id,
            pane_id: $observed.pane_id,
            provider_session: $observed.provider_session,
            status: $observed.status,
            revision: $observed.revision
          }
        }
    ' >"$ROOT/binding-debug-$suffix.json"
}

assert_initial_topology() {
  api_json GET "/api/v1/runtimes/herdr/sessions/$SESSION/inventory" '' 200 \
    "$ROOT/inventory.json"
  api_json GET /api/v1/projects '' 200 "$ROOT/projects.json"
  api_json GET /api/v1/workers '' 200 "$ROOT/workers.json"
  api_json GET "/api/v1/projects/$PROJECT_ONE_ID/assignments" '' 200 \
    "$ROOT/assignments-one.json"
  api_json GET "/api/v1/projects/$PROJECT_TWO_ID/assignments" '' 200 \
    "$ROOT/assignments-two.json"

  jq -e \
    --arg p1 "$PROJECT_ONE_ID" \
    --arg p2 "$PROJECT_TWO_ID" \
    --arg w1 "$WORKSPACE_ONE_ID" \
    --arg w2 "$WORKSPACE_TWO_ID" '
      (.projects | length) == 2
      and ([.projects[].id] | sort) == ([$p1, $p2] | sort)
      and ([.projects[].runtime.workspace_id] | unique | length) == 2
      and ([.projects[].orchestrator.id] | unique | length) == 2
      and ([.projects[].orchestrator.runtime.terminal_id] | unique | length) == 2
      and (any(.projects[]; .id == $p1 and .runtime.workspace_id == $w1))
      and (any(.projects[]; .id == $p2 and .runtime.workspace_id == $w2))
    ' "$ROOT/projects.json" >/dev/null ||
    fail "project/orchestrator topology is not unique"

  jq -e \
    --arg p1 "$PROJECT_ONE_ID" \
    --arg p2 "$PROJECT_TWO_ID" '
      ([.workers[] | select(.availability == "orchestrator" and .project_id == $p1)]
        | length) == 1
      and
      ([.workers[] | select(.availability == "orchestrator" and .project_id == $p2)]
        | length) == 1
      and ([.workers[].worker.runtime.terminal_id] | unique | length) == 4
      and ([.workers[].worker.runtime.pane_id] | unique | length) == 4
    ' "$ROOT/workers.json" >/dev/null ||
    fail "Yard does not expose exactly one uniquely bound orchestrator per project"

  jq -e \
    --arg profile "$PROFILE_ID" \
    --arg workspace "$WORKSPACE_ONE_ID" \
    --arg worker "$WORKER_ONE_ID" '
      (.assignments | length) == 1
      and .assignments[0].profile_id == $profile
      and .assignments[0].worker.profile_id == $profile
      and .assignments[0].worker.id == $worker
      and .assignments[0].worker.runtime.workspace_id == $workspace
      and .assignments[0].lifecycle == "active"
    ' "$ROOT/assignments-one.json" >/dev/null ||
    fail "project one profile-backed worker is invalid"

  jq -e \
    --arg profile "$PROFILE_ID" \
    --arg workspace "$WORKSPACE_TWO_ID" \
    --arg worker "$WORKER_TWO_ID" '
      (.assignments | length) == 1
      and .assignments[0].profile_id == $profile
      and .assignments[0].worker.profile_id == $profile
      and .assignments[0].worker.id == $worker
      and .assignments[0].worker.runtime.workspace_id == $workspace
      and .assignments[0].lifecycle == "active"
    ' "$ROOT/assignments-two.json" >/dev/null ||
    fail "project two profile-backed worker is invalid"

  jq -e \
    --arg t1 "$ORCHESTRATOR_ONE_TERMINAL" \
    --arg t2 "$ORCHESTRATOR_TWO_TERMINAL" \
    --arg t3 "$WORKER_ONE_TERMINAL" \
    --arg t4 "$WORKER_TWO_TERMINAL" '
      ([.workers[].terminal_id] | length) >= 4
      and ([.workers[] | select(
        .terminal_id == $t1
        or .terminal_id == $t2
        or .terminal_id == $t3
        or .terminal_id == $t4
      )] | length) == 4
    ' "$ROOT/inventory.json" >/dev/null ||
    fail "four concurrent live agents were not observed"
}

assert_persisted_invariants() {
  api_json GET /health '' 200 "$ROOT/health.json"
  api_json GET /api/v1/worker-profiles '' 200 "$ROOT/profiles.json"
  api_json GET /api/v1/projects '' 200 "$ROOT/projects.json"
  api_json GET /api/v1/workers '' 200 "$ROOT/workers.json"
  api_json GET "/api/v1/projects/$PROJECT_ONE_ID/assignments" '' 200 \
    "$ROOT/assignments-one.json"
  api_json GET "/api/v1/projects/$PROJECT_TWO_ID/assignments" '' 200 \
    "$ROOT/assignments-two.json"
  api_json GET "/api/v1/runtimes/herdr/sessions/$SESSION/inventory" '' 200 \
    "$ROOT/inventory.json"
  api_json GET \
    "/api/v1/projects/$PROJECT_ONE_ID/assignments/$ASSIGNMENT_ONE_ID/artifacts/$ARTIFACT_ID/content" \
    '' 200 "$ROOT/artifact-content.json"

  jq -e '.status == "ok"' "$ROOT/health.json" >/dev/null ||
    fail "health response changed during soak"
  jq -e --arg profile "$PROFILE_ID" '
    (.profiles | length) == 1
    and .profiles[0].id == $profile
    and .profiles[0].version == "1"
  ' "$ROOT/profiles.json" >/dev/null ||
    fail "profile persistence or uniqueness invariant failed"
  jq -e \
    --arg p1 "$PROJECT_ONE_ID" \
    --arg p2 "$PROJECT_TWO_ID" \
    --arg w1 "$WORKSPACE_ONE_ID" \
    --arg w2 "$WORKSPACE_TWO_ID" \
    --arg o1 "$ORCHESTRATOR_ONE_ID" \
    --arg o2 "$ORCHESTRATOR_TWO_ID" '
      (.projects | length) == 2
      and ([.projects[].id] | sort) == ([$p1, $p2] | sort)
      and ([.projects[].runtime.workspace_id] | unique | length) == 2
      and ([.projects[].orchestrator.id] | unique | length) == 2
      and (any(.projects[];
        .id == $p1
        and .runtime.workspace_id == $w1
        and .orchestrator.id == $o1
        and .placement.geometry == {"x":40.0,"y":60.0,"width":360.0,"height":240.0}))
      and (any(.projects[];
        .id == $p2
        and .runtime.workspace_id == $w2
        and .orchestrator.id == $o2
        and .placement.geometry == {"x":520.0,"y":60.0,"width":360.0,"height":240.0}))
    ' "$ROOT/projects.json" >/dev/null ||
    fail "project, orchestrator, workspace, or placement persistence invariant failed"
  jq -e \
    --arg p1 "$PROJECT_ONE_ID" \
    --arg p2 "$PROJECT_TWO_ID" \
    --arg direct_session "$WORKER_ONE_SESSION_ID" '
      (.workers | length) == 4
      and ([.workers[].worker.id] | unique | length) == 4
      and ([.workers[].worker.runtime.terminal_id] | unique | length) == 4
      and ([.workers[] | select(.availability == "orchestrator" and .project_id == $p1)]
        | length) == 1
      and ([.workers[] | select(.availability == "orchestrator" and .project_id == $p2)]
        | length) == 1
      and ([.workers[] | select(
        .worker.runtime.provider_session.value == $direct_session)]
        | length) == 1
    ' "$ROOT/workers.json" >/dev/null ||
    fail "worker uniqueness or reconciliation invariant failed"
  jq -e \
    --arg assignment "$ASSIGNMENT_ONE_ID" \
    --arg receipt "$RECEIPT_ID" \
    --arg artifact "$ARTIFACT_ID" '
      (.assignments | length) == 1
      and .assignments[0].id == $assignment
      and .assignments[0].lifecycle == "completed"
      and .assignments[0].attempt.lifecycle == "completed"
      and .assignments[0].completion_receipt.id == $receipt
      and .assignments[0].completion_receipt.artifacts[0].id == $artifact
    ' "$ROOT/assignments-one.json" >/dev/null ||
    fail "completion receipt persistence invariant failed"
  jq -e \
    --arg assignment "$ASSIGNMENT_TWO_ID" \
    --arg worker "$WORKER_TWO_ID" '
      (.assignments | length) == 1
      and .assignments[0].id == $assignment
      and .assignments[0].worker.id == $worker
      and .assignments[0].lifecycle == "active"
      and .assignments[0].attempt.lifecycle == "active"
      and .assignments[0].completion_receipt == null
      and .assignments[0].worker.runtime.observation_state == "missing"
      and .assignments[0].worker.runtime.process_state == "unknown"
    ' "$ROOT/assignments-two.json" >/dev/null ||
    fail "process exit incorrectly changed assignment lifecycle"
  jq -e \
    --arg content "$ARTIFACT_CONTENT" \
    --arg artifact "$ARTIFACT_ID" '
      .artifact.id == $artifact
      and .artifact.kind == "markdown"
      and .content == $content
    ' "$ROOT/artifact-content.json" >/dev/null ||
    fail "managed artifact content is not inspectable"
  jq -e \
    --arg w1 "$WORKSPACE_ONE_ID" \
    --arg w2 "$WORKSPACE_TWO_ID" \
    --arg direct_session "$WORKER_ONE_SESSION_ID" \
    --arg exited_terminal "$WORKER_TWO_TERMINAL" \
    --argjson expected_workspaces "$EXPECTED_WORKSPACE_COUNT" '
      (.workspaces | length) == $expected_workspaces
      and ([.workspaces[] | select(
        .runtime_id == $w1 or .runtime_id == $w2)] | length) == 2
      and (.workers | length) == 3
      and ([.panes[] | select(
        .provider_session.value == $direct_session)] | length) == 1
      and ([.panes[] | select(
        .terminal_id == $exited_terminal)] | length) == 0
      and ([.workers[] | select(
        .provider_session.value == $direct_session)] | length) == 1
      and ([.workers[] | select(
        .terminal_id == $exited_terminal)] | length) == 0
    ' "$ROOT/inventory.json" >/dev/null ||
    fail "Herdr topology or exited-process observation invariant failed"
}

for command in cargo curl git herdr jq node rg; do
  require_command "$command"
done
node -e 'require(process.argv[1])' "$WS_MODULE" >/dev/null 2>&1 ||
  fail "Node module 'ws' is required for the terminal WebSocket acceptance check"
[[ "$DURATION_SECONDS" =~ ^[0-9]+$ ]] ||
  fail "YARD_V1_ACCEPTANCE_DURATION_SECONDS must be a non-negative integer"
if [[ -n "$PORT" ]]; then
  if [[ ! "$PORT" =~ ^[0-9]+$ ]] || ((PORT < 1 || PORT > 65535)); then
    fail "YARD_V1_ACCEPTANCE_PORT must be a valid TCP port"
  fi
else
  PORT=$(node -e '
    const server = require("node:net").createServer();
    server.listen(0, "127.0.0.1", () => {
      console.log(server.address().port);
      server.close();
    });
  ')
fi
API_BASE="http://127.0.0.1:$PORT"

mkdir -p \
  "$XDG_CONFIG_HOME" \
  "$XDG_STATE_HOME" \
  "$XDG_DATA_HOME" \
  "$XDG_CACHE_HOME" \
  "$XDG_RUNTIME_DIR" \
  "$ROOT/herdr-startup" \
  "$ROOT/repository-one" \
  "$ROOT/repository-two" \
  "$ROOT/artifacts"
chmod 700 "$XDG_RUNTIME_DIR"

for repository in "$ROOT/repository-one" "$ROOT/repository-two"; do
  git -C "$repository" init -q
  git -C "$repository" config user.name "Yard v1 acceptance"
  git -C "$repository" config user.email "yard-v1-acceptance@invalid"
  printf '# %s\n' "$(basename "$repository")" >"$repository/README.md"
  git -C "$repository" add README.md
  git -C "$repository" commit -qm 'Initialize isolated acceptance repository'
done
REPOSITORY_ONE_COMMON=$(git -C "$ROOT/repository-one" rev-parse --absolute-git-dir)
REPOSITORY_TWO_COMMON=$(git -C "$ROOT/repository-two" rev-parse --absolute-git-dir)
[[ "$REPOSITORY_ONE_COMMON" != "$REPOSITORY_TWO_COMMON" ]] ||
  fail "temporary repositories are not independent"

note "building Yard and starting isolated services"
(
  cd "$REPO_ROOT"
  cargo build --locked -p yard-server
) >"$ROOT/cargo-build.log" 2>&1 || fail "cargo build failed"
start_herdr
BASELINE_WORKSPACE_COUNT=$(herdr --session "$SESSION" workspace list |
  jq -er '.result.workspaces | length')
start_yard

PROFILE_BODY=$(jq -nc '{
  name: "Yard v1 acceptance Codex",
  runtime_adapter: "herdr",
  provider: "codex",
  model: null,
  default_role: "acceptance",
  instructions_ref: null,
  tools: [],
  skills: [],
  mcp_servers: [],
  sandbox_policy: "runtime_default",
  worktree_policy: "project_workspace",
  permission_policy: "yolo",
  completion_contract: "manual_receipt"
}')
api_json POST /api/v1/worker-profiles "$PROFILE_BODY" 201 "$ROOT/profile.json"
PROFILE_ID=$(jq -er '.id' "$ROOT/profile.json")
PROFILE_VERSION=$(jq -er '.version' "$ROOT/profile.json")

PROJECT_ONE_COMMAND=$(uuid)
PROJECT_ONE_BODY=$(jq -nc \
  --arg command_id "$PROJECT_ONE_COMMAND" \
  --arg session "$SESSION" \
  --arg cwd "$ROOT/repository-one" \
  --arg profile_id "$PROFILE_ID" \
  --arg profile_version "$PROFILE_VERSION" '{
    command_id: $command_id,
    actor: "live-v1-acceptance",
    name: "Acceptance project one",
    runtime_adapter: "herdr",
    runtime_session: $session,
    workspace_label: "Acceptance project one",
    cwd: $cwd,
    profile_id: $profile_id,
    expected_profile_version: $profile_version,
    orchestrator_objective: "Reply exactly: YARD_V1_ORCHESTRATOR_ONE_READY",
    placement: {x: 40, y: 60, width: 360, height: 240}
  }')
api_json POST /api/v1/projects/from-profile/workspace "$PROJECT_ONE_BODY" 201 \
  "$ROOT/project-one-created.json"
PROJECT_ONE_ID=$(jq -er '.project.id' "$ROOT/project-one-created.json")
PROJECT_ONE_VERSION=$(jq -er '.project.version' "$ROOT/project-one-created.json")
WORKSPACE_ONE_ID=$(jq -er '.project.runtime.workspace_id' "$ROOT/project-one-created.json")
ORCHESTRATOR_ONE_ID=$(jq -er '.project.orchestrator.id' "$ROOT/project-one-created.json")
ORCHESTRATOR_ONE_TERMINAL=$(jq -er \
  '.project.orchestrator.runtime.terminal_id' "$ROOT/project-one-created.json")
ORCHESTRATOR_ONE_PANE=$(jq -er \
  '.project.orchestrator.runtime.pane_id' "$ROOT/project-one-created.json")
WORKSPACE_IDS+=("$WORKSPACE_ONE_ID")

PROJECT_TWO_COMMAND=$(uuid)
PROJECT_TWO_BODY=$(jq -nc \
  --arg command_id "$PROJECT_TWO_COMMAND" \
  --arg session "$SESSION" \
  --arg cwd "$ROOT/repository-two" \
  --arg profile_id "$PROFILE_ID" \
  --arg profile_version "$PROFILE_VERSION" '{
    command_id: $command_id,
    actor: "live-v1-acceptance",
    name: "Acceptance project two",
    runtime_adapter: "herdr",
    runtime_session: $session,
    workspace_label: "Acceptance project two",
    cwd: $cwd,
    profile_id: $profile_id,
    expected_profile_version: $profile_version,
    orchestrator_objective: "Reply exactly: YARD_V1_ORCHESTRATOR_TWO_READY",
    placement: {x: 520, y: 60, width: 360, height: 240}
  }')
api_json POST /api/v1/projects/from-profile/workspace "$PROJECT_TWO_BODY" 201 \
  "$ROOT/project-two-created.json"
PROJECT_TWO_ID=$(jq -er '.project.id' "$ROOT/project-two-created.json")
PROJECT_TWO_VERSION=$(jq -er '.project.version' "$ROOT/project-two-created.json")
WORKSPACE_TWO_ID=$(jq -er '.project.runtime.workspace_id' "$ROOT/project-two-created.json")
ORCHESTRATOR_TWO_ID=$(jq -er '.project.orchestrator.id' "$ROOT/project-two-created.json")
ORCHESTRATOR_TWO_TERMINAL=$(jq -er \
  '.project.orchestrator.runtime.terminal_id' "$ROOT/project-two-created.json")
ORCHESTRATOR_TWO_PANE=$(jq -er \
  '.project.orchestrator.runtime.pane_id' "$ROOT/project-two-created.json")
WORKSPACE_IDS+=("$WORKSPACE_TWO_ID")

[[ "$WORKSPACE_ONE_ID" != "$WORKSPACE_TWO_ID" ]] ||
  fail "project workspaces are not unique"
EXPECTED_WORKSPACE_COUNT=$((BASELINE_WORKSPACE_COUNT + 2))
ACTUAL_WORKSPACE_COUNT=$(herdr --session "$SESSION" workspace list |
  jq -er '.result.workspaces | length')
[[ "$ACTUAL_WORKSPACE_COUNT" == "$EXPECTED_WORKSPACE_COUNT" ]] ||
  fail "expected exactly two new Herdr workspaces"

herdr --session "$SESSION" agent prompt "$ORCHESTRATOR_ONE_PANE" \
  "Reply exactly: YARD_V1_ORCHESTRATOR_ONE_SESSION_READY" \
  --wait \
  --timeout 180000 >"$ROOT/orchestrator-one-session-prompt.json"
herdr --session "$SESSION" agent prompt "$ORCHESTRATOR_TWO_PANE" \
  "Reply exactly: YARD_V1_ORCHESTRATOR_TWO_SESSION_READY" \
  --wait \
  --timeout 180000 >"$ROOT/orchestrator-two-session-prompt.json"
api_json GET "/api/v1/runtimes/herdr/sessions/$SESSION/inventory" '' 200 \
  "$ROOT/orchestrator-session-inventory.json"
jq -e \
  --arg one "$ORCHESTRATOR_ONE_TERMINAL" \
  --arg two "$ORCHESTRATOR_TWO_TERMINAL" '
    ([.workers[] | select(
      (.terminal_id == $one or .terminal_id == $two)
      and .provider_session != null
    )] | length) == 2
  ' "$ROOT/orchestrator-session-inventory.json" >/dev/null ||
  fail "orchestrator provider-session identities were not established"

note "creating direct Herdr workers for reconciliation"
api_json GET /api/v1/workers '' 200 "$ROOT/workers-before-direct.json"
WORKERS_BEFORE_DIRECT=$(jq -er '.workers | length' "$ROOT/workers-before-direct.json")
[[ "$WORKERS_BEFORE_DIRECT" == 2 ]] ||
  fail "unexpected durable worker count before direct Herdr creation"

herdr --session "$SESSION" tab create \
  --workspace "$WORKSPACE_ONE_ID" \
  --cwd "$ROOT/repository-one" \
  --label "Direct worker one" \
  --no-focus >"$ROOT/direct-tab-one.json"
WORKER_ONE_PANE=$(jq -er '.result.root_pane.pane_id' "$ROOT/direct-tab-one.json")
WORKER_ONE_TERMINAL=$(jq -er '.result.root_pane.terminal_id' "$ROOT/direct-tab-one.json")
DIRECT_AGENT_NAME_ONE="yardone${RUN_ID:0:10}"
start_direct_agent "$DIRECT_AGENT_NAME_ONE" "$WORKER_ONE_PANE" \
  "$ROOT/direct-agent-one.json"
herdr --session "$SESSION" agent prompt "$WORKER_ONE_PANE" \
  "Reply exactly: YARD_V1_DIRECT_ONE_READY" \
  --wait \
  --timeout 180000 >"$ROOT/direct-agent-one-prompt.json"

herdr --session "$SESSION" tab create \
  --workspace "$WORKSPACE_TWO_ID" \
  --cwd "$ROOT/repository-two" \
  --label "Direct worker two" \
  --no-focus >"$ROOT/direct-tab-two.json"
WORKER_TWO_PANE=$(jq -er '.result.root_pane.pane_id' "$ROOT/direct-tab-two.json")
WORKER_TWO_TERMINAL=$(jq -er '.result.root_pane.terminal_id' "$ROOT/direct-tab-two.json")
DIRECT_AGENT_NAME_TWO="yardtwo${RUN_ID:0:10}"
start_direct_agent "$DIRECT_AGENT_NAME_TWO" "$WORKER_TWO_PANE" \
  "$ROOT/direct-agent-two.json"
herdr --session "$SESSION" agent prompt "$WORKER_TWO_PANE" \
  "Reply exactly: YARD_V1_DIRECT_TWO_READY" \
  --wait \
  --timeout 180000 >"$ROOT/direct-agent-two-prompt.json"

RECONCILIATION_START_MS=$(now_ms)
api_json GET "/api/v1/runtimes/herdr/sessions/$SESSION/inventory" '' 200 \
  "$ROOT/direct-inventory.json"
api_json GET /api/v1/workers '' 200 "$ROOT/workers-after-direct.json"
RECONCILIATION_END_MS=$(now_ms)
RECONCILIATION_ELAPSED_MS=$((RECONCILIATION_END_MS - RECONCILIATION_START_MS))
[[ "$RECONCILIATION_ELAPSED_MS" -lt 1000 ]] ||
  fail "observed reconciliation transition took ${RECONCILIATION_ELAPSED_MS}ms"
if ! jq -e \
  --arg terminal_one "$WORKER_ONE_TERMINAL" \
  --arg terminal_two "$WORKER_TWO_TERMINAL" '
    (.workers | length) == 4
    and ([.workers[] | select(
      .worker.runtime.terminal_id == $terminal_one
      and .worker.profile_id == null
      and .availability == "unassigned_live"
    )] | length) == 1
    and ([.workers[] | select(
      .worker.runtime.terminal_id == $terminal_two
      and .worker.profile_id == null
      and .availability == "unassigned_live"
    )] | length) == 1
  ' "$ROOT/workers-after-direct.json" >/dev/null; then
  jq '[.workers[] | {
    terminal_id: .worker.runtime.terminal_id,
    profile_id: .worker.profile_id,
    availability,
    observation_state: .worker.runtime.observation_state,
    process_state: .worker.runtime.process_state,
    provider_session: .worker.runtime.provider_session
  }]' "$ROOT/workers-after-direct.json" >&2
  fail "direct Herdr workers were not reconciled exactly once"
fi

WORKER_ONE_ID=$(jq -er \
  --arg terminal "$WORKER_ONE_TERMINAL" '
    .workers[]
    | select(.worker.runtime.terminal_id == $terminal)
    | .worker.id
  ' "$ROOT/workers-after-direct.json")
WORKER_ONE_VERSION=$(jq -er \
  --arg terminal "$WORKER_ONE_TERMINAL" '
    .workers[]
    | select(.worker.runtime.terminal_id == $terminal)
    | .worker.version
  ' "$ROOT/workers-after-direct.json")
WORKER_TWO_ID=$(jq -er \
  --arg terminal "$WORKER_TWO_TERMINAL" '
    .workers[]
    | select(.worker.runtime.terminal_id == $terminal)
    | .worker.id
  ' "$ROOT/workers-after-direct.json")
WORKER_TWO_VERSION=$(jq -er \
  --arg terminal "$WORKER_TWO_TERMINAL" '
    .workers[]
    | select(.worker.runtime.terminal_id == $terminal)
    | .worker.version
  ' "$ROOT/workers-after-direct.json")

api_json GET "/api/v1/runtimes/herdr/sessions/$SESSION/inventory" '' 200 \
  "$ROOT/direct-inventory-replay.json"
api_json GET /api/v1/workers '' 200 "$ROOT/workers-after-direct-replay.json"
jq -e \
  --arg terminal_one "$WORKER_ONE_TERMINAL" \
  --arg terminal_two "$WORKER_TWO_TERMINAL" '
    (.workers | length) == 4
    and ([.workers[] | select(.worker.runtime.terminal_id == $terminal_one)] | length) == 1
    and ([.workers[] | select(.worker.runtime.terminal_id == $terminal_two)] | length) == 1
  ' "$ROOT/workers-after-direct-replay.json" >/dev/null ||
  fail "repeated reconciliation duplicated a direct Herdr worker"
WORKER_ONE_VERSION=$(jq -er \
  --arg terminal "$WORKER_ONE_TERMINAL" '
    .workers[]
    | select(.worker.runtime.terminal_id == $terminal)
    | .worker.version
  ' "$ROOT/workers-after-direct-replay.json")
WORKER_TWO_VERSION=$(jq -er \
  --arg terminal "$WORKER_TWO_TERMINAL" '
    .workers[]
    | select(.worker.runtime.terminal_id == $terminal)
    | .worker.version
  ' "$ROOT/workers-after-direct-replay.json")

api_json GET "/api/v1/projects/$PROJECT_ONE_ID" '' 200 "$ROOT/project-one.json"
PROJECT_ONE_VERSION=$(jq -er '.version' "$ROOT/project-one.json")
api_json GET "/api/v1/projects/$PROJECT_TWO_ID" '' 200 "$ROOT/project-two.json"
PROJECT_TWO_VERSION=$(jq -er '.version' "$ROOT/project-two.json")

note "allocating one reconciled profile-backed worker to each project"
ALLOCATION_ONE_COMMAND=$(uuid)
ALLOCATION_ONE_BODY=$(jq -nc \
  --arg command_id "$ALLOCATION_ONE_COMMAND" \
  --arg worker_id "$WORKER_ONE_ID" \
  --arg worker_version "$WORKER_ONE_VERSION" \
  --arg profile_id "$PROFILE_ID" \
  --arg profile_version "$PROFILE_VERSION" \
  --arg project_version "$PROJECT_ONE_VERSION" '{
    command_id: $command_id,
    actor: "live-v1-acceptance",
    worker_id: $worker_id,
    expected_worker_version: $worker_version,
    profile_id: $profile_id,
    expected_profile_version: $profile_version,
    expected_project_version: $project_version,
    objective: "Reply exactly: YARD_V1_WORKER_ONE_READY",
    role: "acceptance worker",
    isolation_policy: "project_workspace"
  }')
api_json POST "/api/v1/projects/$PROJECT_ONE_ID/assignments" "$ALLOCATION_ONE_BODY" 201 \
  "$ROOT/allocation-one.json"
ASSIGNMENT_ONE_ID=$(jq -er '.assignment.id' "$ROOT/allocation-one.json")
ASSIGNMENT_ONE_VERSION=$(jq -er '.assignment.version' "$ROOT/allocation-one.json")
ATTEMPT_ONE_ID=$(jq -er '.assignment.attempt.id' "$ROOT/allocation-one.json")
ATTEMPT_ONE_VERSION=$(jq -er '.assignment.attempt.version' "$ROOT/allocation-one.json")
jq -e \
  --arg worker "$WORKER_ONE_ID" \
  --arg terminal "$WORKER_ONE_TERMINAL" \
  --arg profile "$PROFILE_ID" '
    .allocation.mode == "adopt_existing"
    and .assignment.worker.id == $worker
    and .assignment.worker.profile_id == $profile
    and .assignment.worker.runtime.terminal_id == $terminal
  ' "$ROOT/allocation-one.json" >/dev/null ||
  fail "project one did not adopt the reconciled direct worker"

ALLOCATION_TWO_COMMAND=$(uuid)
ALLOCATION_TWO_BODY=$(jq -nc \
  --arg command_id "$ALLOCATION_TWO_COMMAND" \
  --arg worker_id "$WORKER_TWO_ID" \
  --arg worker_version "$WORKER_TWO_VERSION" \
  --arg profile_id "$PROFILE_ID" \
  --arg profile_version "$PROFILE_VERSION" \
  --arg project_version "$PROJECT_TWO_VERSION" '{
    command_id: $command_id,
    actor: "live-v1-acceptance",
    worker_id: $worker_id,
    expected_worker_version: $worker_version,
    profile_id: $profile_id,
    expected_profile_version: $profile_version,
    expected_project_version: $project_version,
    objective: "Reply exactly: YARD_V1_WORKER_TWO_READY",
    role: "acceptance worker",
    isolation_policy: "project_workspace"
  }')
api_json POST "/api/v1/projects/$PROJECT_TWO_ID/assignments" "$ALLOCATION_TWO_BODY" 201 \
  "$ROOT/allocation-two.json"
ASSIGNMENT_TWO_ID=$(jq -er '.assignment.id' "$ROOT/allocation-two.json")
jq -e \
  --arg worker "$WORKER_TWO_ID" \
  --arg terminal "$WORKER_TWO_TERMINAL" \
  --arg profile "$PROFILE_ID" '
    .allocation.mode == "adopt_existing"
    and .assignment.worker.id == $worker
    and .assignment.worker.profile_id == $profile
    and .assignment.worker.runtime.terminal_id == $terminal
  ' "$ROOT/allocation-two.json" >/dev/null ||
  fail "project two did not adopt the reconciled direct worker"

capture_orchestrator_binding "$PROJECT_ONE_ID" one
capture_orchestrator_binding "$PROJECT_TWO_ID" two
wait_for_exact_output_line \
  "/api/v1/projects/$PROJECT_ONE_ID/orchestrator/terminal-output?lines=240" \
  YARD_V1_ORCHESTRATOR_ONE_SESSION_READY
wait_for_exact_output_line \
  "/api/v1/projects/$PROJECT_TWO_ID/orchestrator/terminal-output?lines=240" \
  YARD_V1_ORCHESTRATOR_TWO_SESSION_READY
wait_for_exact_output_line \
  "/api/v1/projects/$PROJECT_ONE_ID/assignments/$ASSIGNMENT_ONE_ID/terminal-output?lines=240" \
  YARD_V1_WORKER_ONE_READY
wait_for_exact_output_line \
  "/api/v1/projects/$PROJECT_TWO_ID/assignments/$ASSIGNMENT_TWO_ID/terminal-output?lines=240" \
  YARD_V1_WORKER_TWO_READY
assert_initial_topology

note "verifying direct worker and orchestrator prompts"
WORKER_PROMPT_COMMAND=$(uuid)
WORKER_PROMPT_BODY=$(jq -nc \
  --arg command_id "$WORKER_PROMPT_COMMAND" \
  --arg attempt_id "$ATTEMPT_ONE_ID" \
  --arg assignment_version "$ASSIGNMENT_ONE_VERSION" \
  --arg attempt_version "$ATTEMPT_ONE_VERSION" '{
    command_id: $command_id,
    actor: "live-v1-acceptance",
    attempt_id: $attempt_id,
    expected_assignment_version: $assignment_version,
    expected_attempt_version: $attempt_version,
    text: "Reply exactly: YARD_V1_WORKER_DIRECT_PROMPT_OK"
  }')
STATUS_TRANSITION_START_MS=$(now_ms)
api_json POST \
  "/api/v1/projects/$PROJECT_ONE_ID/assignments/$ASSIGNMENT_ONE_ID/prompts" \
  "$WORKER_PROMPT_BODY" 200 "$ROOT/worker-prompt.json"
STATUS_TRANSITION_END_MS=$(now_ms)
STATUS_TRANSITION_ELAPSED_MS=$((STATUS_TRANSITION_END_MS - STATUS_TRANSITION_START_MS))
[[ "$STATUS_TRANSITION_ELAPSED_MS" -lt 1000 ]] ||
  fail "observed worker status transition took ${STATUS_TRANSITION_ELAPSED_MS}ms"
wait_for_exact_output_line \
  "/api/v1/projects/$PROJECT_ONE_ID/assignments/$ASSIGNMENT_ONE_ID/terminal-output?lines=240" \
  YARD_V1_WORKER_DIRECT_PROMPT_OK

api_json GET "/api/v1/projects/$PROJECT_TWO_ID" '' 200 "$ROOT/project-two.json"
PROJECT_TWO_CURRENT_VERSION=$(jq -er '.version' "$ROOT/project-two.json")
ORCHESTRATOR_PROMPT_COMMAND=$(uuid)
ORCHESTRATOR_PROMPT_BODY=$(jq -nc \
  --arg command_id "$ORCHESTRATOR_PROMPT_COMMAND" \
  --arg project_version "$PROJECT_TWO_CURRENT_VERSION" \
  --arg worker_id "$ORCHESTRATOR_TWO_ID" '{
    command_id: $command_id,
    actor: "live-v1-acceptance",
    expected_project_version: $project_version,
    orchestrator_worker_id: $worker_id,
    text: "Reply exactly: YARD_V1_ORCHESTRATOR_DIRECT_PROMPT_OK"
  }')
api_json POST "/api/v1/projects/$PROJECT_TWO_ID/orchestrator/prompts" \
  "$ORCHESTRATOR_PROMPT_BODY" 200 "$ROOT/orchestrator-prompt.json"
wait_for_exact_output_line \
  "/api/v1/projects/$PROJECT_TWO_ID/orchestrator/terminal-output?lines=240" \
  YARD_V1_ORCHESTRATOR_DIRECT_PROMPT_OK

note "verifying the live terminal WebSocket"
WS_URL="ws://127.0.0.1:$PORT/api/v1/projects/$PROJECT_ONE_ID/assignments/$ASSIGNMENT_ONE_ID/terminal?cols=80&rows=24"
node - "$WS_URL" "$WS_MODULE" >"$ROOT/websocket.json" <<'NODE'
const WebSocket = require(process.argv[3]);

const url = process.argv[2];
const socket = new WebSocket(url, { origin: "http://127.0.0.1:5173" });
let firstFrame = null;
let resized = false;
const timeout = setTimeout(() => {
  console.error("terminal WebSocket timed out");
  socket.terminate();
  process.exit(1);
}, 30000);

socket.on("open", () => {});
socket.on("message", (data, binary) => {
  if (binary) {
    console.error("terminal WebSocket returned a binary message");
    process.exit(1);
  }
  const message = JSON.parse(data.toString());
  if (firstFrame === null) {
    if (message.type !== "terminal.frame" || message.full !== true) {
      console.error("terminal stream did not begin with a full frame");
      process.exit(1);
    }
    Buffer.from(message.bytes, "base64");
    firstFrame = message;
    socket.send(JSON.stringify({
      type: "terminal.resize",
      cols: 90,
      rows: 30,
    }));
    resized = true;
    socket.send(JSON.stringify({ type: "terminal.release" }));
    return;
  }
  if (message.type === "terminal.closed") {
    if (message.reason !== "released") {
      console.error(`terminal closed unexpectedly: ${message.reason}`);
      process.exit(1);
    }
    clearTimeout(timeout);
    process.stdout.write(JSON.stringify({
      full_frame: firstFrame.full,
      sequence: firstFrame.seq,
      resize_sent: resized,
      release_reason: message.reason,
    }));
    socket.close();
  }
});
socket.on("error", (error) => {
  clearTimeout(timeout);
  console.error(error.message);
  process.exit(1);
});
NODE
jq -e '
  .full_frame == true
  and .resize_sent == true
  and .release_reason == "released"
' "$ROOT/websocket.json" >/dev/null ||
  fail "terminal WebSocket did not accept full-frame, resize, and release flow"

note "uploading, inspecting, and completing with a typed artifact"
ARTIFACT_ID=$(uuid)
ARTIFACT_CONTENT="# Yard v1 acceptance"$'\n\n'"Project: $PROJECT_ONE_ID"$'\n\n'"Result: verified"
ARTIFACT_BODY=$(jq -nc \
  --arg attempt_id "$ATTEMPT_ONE_ID" \
  --arg assignment_version "$ASSIGNMENT_ONE_VERSION" \
  --arg attempt_version "$ATTEMPT_ONE_VERSION" \
  --arg content "$ARTIFACT_CONTENT" '{
    actor: "live-v1-acceptance",
    attempt_id: $attempt_id,
    expected_assignment_version: $assignment_version,
    expected_attempt_version: $attempt_version,
    kind: "markdown",
    display_name: "yard-v1-acceptance.md",
    content: $content
  }')
ARTIFACT_PATH="/api/v1/projects/$PROJECT_ONE_ID/assignments/$ASSIGNMENT_ONE_ID/artifacts/$ARTIFACT_ID"
api_json PUT "$ARTIFACT_PATH" "$ARTIFACT_BODY" 201 "$ROOT/artifact.json"
api_json GET "$ARTIFACT_PATH" '' 200 "$ROOT/artifact-metadata.json"
api_json GET "$ARTIFACT_PATH/content" '' 200 "$ROOT/artifact-content.json"
jq -e \
  --arg artifact "$ARTIFACT_ID" \
  --arg assignment "$ASSIGNMENT_ONE_ID" \
  --arg content "$ARTIFACT_CONTENT" '
    .artifact.id == $artifact
    and .artifact.assignment_id == $assignment
    and .artifact.kind == "markdown"
    and .artifact.media_type == "text/markdown"
    and .content == $content
  ' "$ROOT/artifact-content.json" >/dev/null ||
  fail "uploaded artifact could not be inspected"
ARTIFACT_SHA256=$(jq -er '.sha256' "$ROOT/artifact-metadata.json")
mapfile -t MANAGED_ARTIFACT_FILES < <(find "$ROOT/artifacts" -type f -print)
[[ "${#MANAGED_ARTIFACT_FILES[@]}" == 1 ]] ||
  fail "artifact store does not contain exactly one managed object"
MANAGED_ARTIFACT_SHA256=$(sha256sum "${MANAGED_ARTIFACT_FILES[0]}" | awk '{print $1}')
[[ "$MANAGED_ARTIFACT_SHA256" == "$ARTIFACT_SHA256" ]] ||
  fail "managed artifact hash does not match metadata"

COMPLETION_COMMAND=$(uuid)
COMPLETION_BODY=$(jq -nc \
  --arg command_id "$COMPLETION_COMMAND" \
  --arg attempt_id "$ATTEMPT_ONE_ID" \
  --arg assignment_version "$ASSIGNMENT_ONE_VERSION" \
  --arg attempt_version "$ATTEMPT_ONE_VERSION" \
  --arg artifact_id "$ARTIFACT_ID" '{
    command_id: $command_id,
    actor: "live-v1-acceptance",
    attempt_id: $attempt_id,
    expected_assignment_version: $assignment_version,
    expected_attempt_version: $attempt_version,
    outcome: "completed",
    summary: "Yard v1 acceptance artifact was uploaded and inspected.",
    artifact_refs: [],
    artifact_ids: [$artifact_id],
    evidence_refs: ["acceptance://live-v1"],
    unresolved_blockers: []
  }')
api_json POST \
  "/api/v1/projects/$PROJECT_ONE_ID/assignments/$ASSIGNMENT_ONE_ID/completion-receipts" \
  "$COMPLETION_BODY" 201 "$ROOT/completion.json"
RECEIPT_ID=$(jq -er '.receipt.id' "$ROOT/completion.json")
jq -e \
  --arg receipt "$RECEIPT_ID" \
  --arg artifact "$ARTIFACT_ID" '
    .receipt.id == $receipt
    and .receipt.actor == "live-v1-acceptance"
    and .receipt.artifacts[0].id == $artifact
    and .assignment.lifecycle == "completed"
    and .assignment.attempt.lifecycle == "completed"
  ' "$ROOT/completion.json" >/dev/null ||
  fail "completion receipt is not auditable"

note "exiting a worker process without completing its assignment"
api_json GET "/api/v1/projects/$PROJECT_ONE_ID" '' 200 "$ROOT/pre-restart-project-one.json"
api_json GET "/api/v1/projects/$PROJECT_TWO_ID" '' 200 "$ROOT/pre-restart-project-two.json"
api_json GET "/api/v1/projects/$PROJECT_ONE_ID/assignments" '' 200 \
  "$ROOT/pre-restart-assignments-one.json"
api_json GET "/api/v1/projects/$PROJECT_TWO_ID/assignments" '' 200 \
  "$ROOT/pre-restart-assignments-two.json"
ORCHESTRATOR_ONE_SESSION_ID=$(jq -er \
  '.orchestrator.runtime.provider_session.value' "$ROOT/pre-restart-project-one.json")
ORCHESTRATOR_TWO_SESSION_ID=$(jq -er \
  '.orchestrator.runtime.provider_session.value' "$ROOT/pre-restart-project-two.json")
WORKER_ONE_SESSION_ID=$(jq -er \
  '.assignments[0].worker.runtime.provider_session.value' \
  "$ROOT/pre-restart-assignments-one.json")
WORKER_TWO_SESSION_ID=$(jq -er \
  '.assignments[0].worker.runtime.provider_session.value' \
  "$ROOT/pre-restart-assignments-two.json")
herdr --session "$SESSION" agent list >"$ROOT/pre-exit-agent-list.json"
WORKER_TWO_AGENT_TARGET=$(jq -er \
  --arg session_id "$WORKER_TWO_SESSION_ID" '
    .result.agents[]
    | select(.agent_session.value == $session_id)
    | .name
  ' "$ROOT/pre-exit-agent-list.json")
if ! herdr --session "$SESSION" agent prompt "$WORKER_TWO_AGENT_TARGET" /exit \
  >"$ROOT/worker-exit-prompt.json"; then
  fail "could not submit the exit command to the reconciled worker"
fi
EXIT_DEADLINE=$((SECONDS + 20))
PROCESS_EXITED=false
while ((SECONDS < EXIT_DEADLINE)); do
  api_json GET "/api/v1/runtimes/herdr/sessions/$SESSION/inventory" '' 200 \
    "$ROOT/exit-inventory.json"
  api_json GET "/api/v1/projects/$PROJECT_TWO_ID/assignments" '' 200 \
    "$ROOT/assignments-two.json"
  if jq -e \
    --arg terminal_id "$WORKER_TWO_TERMINAL" '
      ([.workers[] | select(
        .terminal_id == $terminal_id)] | length) == 0
      and ([.panes[] | select(
        .terminal_id == $terminal_id)] | length) == 1
    ' "$ROOT/exit-inventory.json" >/dev/null &&
    jq -e --arg assignment "$ASSIGNMENT_TWO_ID" '
      .assignments[0].id == $assignment
      and .assignments[0].lifecycle == "active"
      and .assignments[0].attempt.lifecycle == "active"
      and .assignments[0].completion_receipt == null
      and .assignments[0].worker.runtime.process_state == "exited"
    ' "$ROOT/assignments-two.json" >/dev/null; then
    PROCESS_EXITED=true
    break
  fi
  sleep 0.25
done
if [[ "$PROCESS_EXITED" != true ]]; then
  herdr --session "$SESSION" agent send-keys "$WORKER_TWO_AGENT_TARGET" ctrl-c ctrl-c \
    >"$ROOT/worker-exit-fallback.json" || true
  EXIT_DEADLINE=$((SECONDS + 20))
  while ((SECONDS < EXIT_DEADLINE)); do
    api_json GET "/api/v1/runtimes/herdr/sessions/$SESSION/inventory" '' 200 \
      "$ROOT/exit-inventory.json"
    api_json GET "/api/v1/projects/$PROJECT_TWO_ID/assignments" '' 200 \
      "$ROOT/assignments-two.json"
    if jq -e \
      --arg terminal_id "$WORKER_TWO_TERMINAL" '
        ([.workers[] | select(
          .terminal_id == $terminal_id)] | length) == 0
        and ([.panes[] | select(
          .terminal_id == $terminal_id)] | length) == 1
      ' "$ROOT/exit-inventory.json" >/dev/null &&
      jq -e --arg assignment "$ASSIGNMENT_TWO_ID" '
        .assignments[0].id == $assignment
        and .assignments[0].lifecycle == "active"
        and .assignments[0].attempt.lifecycle == "active"
        and .assignments[0].completion_receipt == null
        and .assignments[0].worker.runtime.process_state == "exited"
      ' "$ROOT/assignments-two.json" >/dev/null; then
      PROCESS_EXITED=true
      break
    fi
    sleep 0.25
  done
fi
[[ "$PROCESS_EXITED" == true ]] ||
  fail "worker process did not exit or assignment auto-completion invariant failed"

note "disconnecting and restarting the isolated Herdr service"
stop_herdr_process
curl -fsS --max-time 2 "$API_BASE/health" >/dev/null ||
  fail "Yard health failed while Herdr was disconnected"
DISCONNECT_CODE=$(curl -sS --connect-timeout 2 --max-time 10 \
  -o "$ROOT/disconnected-inventory.json" -w '%{http_code}' \
  "$API_BASE/api/v1/runtimes/herdr/sessions/$SESSION/inventory" || true)
[[ "$DISCONNECT_CODE" != 200 ]] ||
  fail "Yard inventory unexpectedly succeeded while Herdr was disconnected"
start_herdr
api_json GET "/api/v1/runtimes/herdr/sessions/$SESSION/inventory" '' 200 \
  "$ROOT/restored-inventory.json"
RECOVERY_DEADLINE=$((SECONDS + 60))
HERDR_RECOVERED=false
while ((SECONDS < RECOVERY_DEADLINE)); do
  api_json GET "/api/v1/runtimes/herdr/sessions/$SESSION/inventory" '' 200 \
    "$ROOT/recovered-inventory.json"
  if jq -e \
    --arg s1 "$ORCHESTRATOR_ONE_SESSION_ID" \
    --arg s2 "$ORCHESTRATOR_TWO_SESSION_ID" \
    --arg s3 "$WORKER_ONE_SESSION_ID" \
    --arg t4 "$WORKER_TWO_TERMINAL" \
    --arg w1 "$WORKSPACE_ONE_ID" \
    --arg w2 "$WORKSPACE_TWO_ID" \
    --argjson expected_workspaces "$EXPECTED_WORKSPACE_COUNT" '
      (.workers | length) == 3
      and ([.workers[] | select(
        .provider_session.value == $s1
        or .provider_session.value == $s2
        or .provider_session.value == $s3
      )] | length) == 3
      and ([.workers[] | select(
        .terminal_id == $t4)] | length) == 0
      and ([.panes[] | select(
        .terminal_id == $t4)] | length) == 0
      and (.workspaces | length) == $expected_workspaces
      and ([.workspaces[] | select(
        .runtime_id == $w1 or .runtime_id == $w2)] | length) == 2
    ' "$ROOT/recovered-inventory.json" >/dev/null; then
    HERDR_RECOVERED=true
    break
  fi
  sleep 0.25
done
[[ "$HERDR_RECOVERED" == true ]] ||
  fail "Herdr service topology did not recover after restart"

note "restarting Yard and verifying durable persistence"
stop_yard
start_yard
assert_persisted_invariants

note "running ${DURATION_SECONDS}s acceptance soak"
SOAK_START_SECONDS=$(date +%s)
SOAK_DEADLINE_SECONDS=$((SOAK_START_SECONDS + DURATION_SECONDS))
while true; do
  assert_persisted_invariants
  SOAK_POLLS=$((SOAK_POLLS + 1))
  NOW_SECONDS=$(date +%s)
  ((NOW_SECONDS >= SOAK_DEADLINE_SECONDS)) && break
  REMAINING_SECONDS=$((SOAK_DEADLINE_SECONDS - NOW_SECONDS))
  SLEEP_SECONDS=$POLL_SECONDS
  ((REMAINING_SECONDS < SLEEP_SECONDS)) && SLEEP_SECONDS=$REMAINING_SECONDS
  sleep "$SLEEP_SECONDS"
done
SOAK_ELAPSED_SECONDS=$(($(date +%s) - SOAK_START_SECONDS))

jq -n \
  --arg session "$SESSION" \
  --arg artifact_id "$ARTIFACT_ID" \
  --arg receipt_id "$RECEIPT_ID" \
  --argjson duration_seconds "$DURATION_SECONDS" \
  --argjson soak_elapsed_seconds "$SOAK_ELAPSED_SECONDS" \
  --argjson soak_polls "$SOAK_POLLS" \
  --argjson projects 2 \
  --argjson profiles 1 \
  --argjson durable_workers 4 \
  --argjson concurrent_live_agents 4 \
  --argjson workspaces "$EXPECTED_WORKSPACE_COUNT" \
  --argjson assignments 2 \
  --argjson reconciliation_elapsed_ms "$RECONCILIATION_ELAPSED_MS" \
  --argjson status_transition_elapsed_ms "$STATUS_TRANSITION_ELAPSED_MS" \
  --argjson websocket_sequence "$(jq -er '.sequence' "$ROOT/websocket.json")" '{
    ok: true,
    session: $session,
    duration_seconds: $duration_seconds,
    soak_elapsed_seconds: $soak_elapsed_seconds,
    soak_polls: $soak_polls,
    projects: $projects,
    profiles: $profiles,
    durable_workers: $durable_workers,
    concurrent_live_agents_verified: $concurrent_live_agents,
    herdr_workspaces: $workspaces,
    assignments: $assignments,
    completion_receipt: {
      id: $receipt_id,
      artifact_id: $artifact_id,
      auditable: true
    },
    terminal_websocket: {
      full_frame: true,
      resize: true,
      release: true,
      sequence: $websocket_sequence
    },
    process_exit_did_not_complete_assignment: true,
    direct_herdr_reconciliation: {
      unique: true,
      elapsed_ms: $reconciliation_elapsed_ms,
      under_one_second: true
    },
    observed_status_transition: {
      elapsed_ms: $status_transition_elapsed_ms,
      under_one_second: true
    },
    herdr_restart_recovered: true,
    yard_restart_persisted_without_duplication: true
  }'

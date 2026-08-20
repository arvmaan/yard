#!/usr/bin/env bash

set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
BINARY=${1:-"$REPO_ROOT/target/release/yard"}
if [[ "$BINARY" != /* ]]; then
  BINARY="$(cd "$(dirname "$BINARY")" && pwd)/$(basename "$BINARY")"
fi
[[ -x "$BINARY" ]] || {
  printf 'Yard binary is not executable: %s\n' "$BINARY" >&2
  printf 'Build it with: cargo build --release --locked -p yard-server --bin yard\n' >&2
  exit 1
}

ROOT=$(mktemp -d "${TMPDIR:-/tmp}/yard-cli-smoke.XXXXXX")
FOREGROUND_PID=''
UNRELATED_PID=''
SECOND_HOME="$ROOT/second/home"
SECOND_DATA="$ROOT/second/data"
SECOND_STATE="$ROOT/second/state"
SECOND_RUNTIME="$ROOT/second/runtime"
case "$(uname -s)" in
  Darwin) BROWSER_COMMAND=open ;;
  Linux) BROWSER_COMMAND=xdg-open ;;
  *)
    printf 'Yard CLI lifecycle smoke: unsupported platform: %s\n' "$(uname -s)" >&2
    exit 1
    ;;
esac

export HOME="$ROOT/home"
export XDG_DATA_HOME="$ROOT/xdg/data"
export XDG_STATE_HOME="$ROOT/xdg/state"
export XDG_RUNTIME_DIR="$ROOT/xdg/runtime"
export XDG_CONFIG_HOME="$ROOT/xdg/config"
export XDG_CACHE_HOME="$ROOT/xdg/cache"
export YARD_BIND=127.0.0.1:0
export YARD_HERDR_BIN="$ROOT/missing-herdr"
export YARD_ORCHESTRATOR_CWD="$ROOT"

cleanup() {
  local status=$?
  trap - EXIT INT TERM
  "$BINARY" stop >/dev/null 2>&1 || true
  HOME="$SECOND_HOME" \
  XDG_DATA_HOME="$SECOND_DATA" \
  XDG_STATE_HOME="$SECOND_STATE" \
  XDG_RUNTIME_DIR="$SECOND_RUNTIME" \
    "$BINARY" stop >/dev/null 2>&1 || true
  if [[ -n "$FOREGROUND_PID" ]]; then
    kill "$FOREGROUND_PID" >/dev/null 2>&1 || true
    wait "$FOREGROUND_PID" 2>/dev/null || true
  fi
  if [[ -n "$UNRELATED_PID" ]]; then
    kill "$UNRELATED_PID" >/dev/null 2>&1 || true
    wait "$UNRELATED_PID" 2>/dev/null || true
  fi
  find "$ROOT" -depth -delete
  exit "$status"
}
trap cleanup EXIT INT TERM

fail() {
  printf 'yard CLI lifecycle smoke: FAIL: %s\n' "$*" >&2
  find "$XDG_RUNTIME_DIR" -name yard.log -type f -exec tail -n 80 {} \; >&2 || true
  exit 1
}

status_value() {
  local label=$1
  local file=$2
  sed -n "s/^  $label: //p" "$file" | tail -n 1
}

wait_for_exit() {
  local pid=$1
  local _
  for _ in {1..200}; do
    if ! kill -0 "$pid" >/dev/null 2>&1; then
      return
    fi
    sleep 0.05
  done
  fail "process $pid did not exit"
}

file_mode() {
  if stat -c '%a' "$1" >/dev/null 2>&1; then
    stat -c '%a' "$1"
  else
    stat -f '%Lp' "$1"
  fi
}

file_uid() {
  if stat -c '%u' "$1" >/dev/null 2>&1; then
    stat -c '%u' "$1"
  else
    stat -f '%u' "$1"
  fi
}

mkdir -p \
  "$HOME" \
  "$XDG_DATA_HOME" \
  "$XDG_STATE_HOME" \
  "$XDG_RUNTIME_DIR" \
  "$XDG_CONFIG_HOME" \
  "$XDG_CACHE_HOME" \
  "$ROOT/bin"
chmod 700 "$XDG_RUNTIME_DIR"

cat >"$ROOT/bin/$BROWSER_COMMAND" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$1" >>"$YARD_BROWSER_MARKER"
printf '%s\n' "$#" >>"$YARD_BROWSER_ARGC_MARKER"
if [[ -n "${YARD_BROWSER_SLEEP:-}" ]]; then
  printf '%s\n' "$$" >"$YARD_BROWSER_PID"
  exec sleep "$YARD_BROWSER_SLEEP"
fi
exit "${YARD_BROWSER_EXIT:-0}"
EOF
chmod 755 "$ROOT/bin/$BROWSER_COMMAND"
export PATH="$ROOT/bin:$PATH"
export YARD_BROWSER_MARKER="$ROOT/browser-opened"
export YARD_BROWSER_ARGC_MARKER="$ROOT/browser-argc"
export YARD_BROWSER_PID="$ROOT/browser.pid"

set +e
YARD_BIND=not-a-socket \
  "$BINARY" start --no-open >"$ROOT/invalid-bind.out" 2>"$ROOT/invalid-bind.err"
INVALID_BIND_EXIT=$?
set -e
[[ "$INVALID_BIND_EXIT" == 2 ]] ||
  fail "invalid bind returned $INVALID_BIND_EXIT instead of usage exit 2"
[[ ! -e "$XDG_RUNTIME_DIR/yard" ]] ||
  fail "invalid bind published lifecycle state"

bash -c '"$1" start --no-open >"$2"' _ "$BINARY" "$ROOT/start.out"
"$BINARY" status >"$ROOT/status.out"
FIRST_PID=$(status_value PID "$ROOT/status.out")
FIRST_URL=$(status_value URL "$ROOT/status.out")
RUNTIME_LOG=$(status_value Log "$ROOT/status.out")
RUNTIME_ROOT=$(dirname "$RUNTIME_LOG")
[[ -n "$FIRST_PID" && -n "$FIRST_URL" ]] || fail "status omitted PID or URL"
grep -q '^  Mode: managed$' "$ROOT/status.out" ||
  fail "status did not identify the managed owner"
[[ "$(ps -o pgid= -p "$FIRST_PID" | tr -d ' ')" == "$FIRST_PID" ]] ||
  fail "managed child did not own a detached process group"
curl -fsS "${FIRST_URL}health" | grep -q '"status":"ok"' ||
  fail "managed health endpoint was not ready"
curl -fsS "$FIRST_URL" | grep -q '<div id="root"></div>' ||
  fail "managed UI was not ready"
[[ ! -e "$YARD_BROWSER_MARKER" ]] ||
  fail "start --no-open invoked a browser launcher"
[[ "$(file_mode "$RUNTIME_ROOT")" == 700 ]] ||
  fail "runtime directory was not mode 0700"
for private_file in \
  "$RUNTIME_ROOT/instance.json" \
  "$RUNTIME_ROOT/control.sock" \
  "$RUNTIME_ROOT/launch.lock" \
  "$RUNTIME_ROOT/instance.lock" \
  "$RUNTIME_ROOT/yard.log"; do
  [[ "$(file_mode "$private_file")" == 600 ]] ||
    fail "$private_file was not mode 0600"
  [[ "$(file_uid "$private_file")" == "$(id -u)" ]] ||
    fail "$private_file was not owned by the effective user"
done
if grep -q 'control_secret' "$ROOT/status.out" "$RUNTIME_ROOT/yard.log"; then
  fail "control secret field leaked into command output or logs"
fi

"$BINARY" start --no-open >"$ROOT/start-again.out"
SECOND_PID=$(status_value PID "$ROOT/start-again.out")
[[ "$SECOND_PID" == "$FIRST_PID" ]] ||
  fail "second start launched a different managed process"
ln -s "$XDG_DATA_HOME/yard/yard.sqlite3" "$ROOT/database-alias.sqlite3"
YARD_DATABASE_PATH="$ROOT/database-alias.sqlite3" \
  "$BINARY" start --no-open >"$ROOT/alias-start.out"
[[ "$(status_value PID "$ROOT/alias-start.out")" == "$FIRST_PID" ]] ||
  fail "database symlink alias launched a second managed process"
YARD_DATABASE_PATH="$ROOT/database-alias.sqlite3" \
  "$BINARY" status >"$ROOT/alias-status.out"
[[ "$(status_value URL "$ROOT/alias-status.out")" == "$FIRST_URL" ]] ||
  fail "database symlink alias did not resolve to the active lifecycle owner"
ln "$XDG_DATA_HOME/yard/yard.sqlite3" "$ROOT/database-hard-link.sqlite3"
set +e
YARD_DATABASE_PATH="$ROOT/database-hard-link.sqlite3" \
  "$BINARY" start --no-open \
  >"$ROOT/hard-link.out" 2>"$ROOT/hard-link.err"
HARD_LINK_EXIT=$?
set -e
[[ "$HARD_LINK_EXIT" == 2 ]] ||
  fail "hard-linked database returned $HARD_LINK_EXIT instead of configuration failure 2"
grep -q 'hard links' "$ROOT/hard-link.err" ||
  fail "hard-linked database failure was not actionable"
unlink "$ROOT/database-hard-link.sqlite3"
kill -0 "$FIRST_PID" >/dev/null 2>&1 ||
  fail "hard-linked database attempt affected the active owner"
YARD_BIND=127.0.0.1:1 \
  "$BINARY" start --no-open \
  >"$ROOT/different-bind.out" 2>"$ROOT/different-bind.err"
[[ "$(status_value PID "$ROOT/different-bind.out")" == "$FIRST_PID" ]] ||
  fail "different requested bind replaced the running instance"
grep -q '^warning: requested bind ' "$ROOT/different-bind.err" ||
  fail "different requested bind did not warn that the active config won"
YARD_HERDR_BIN="$ROOT/different-herdr" \
  "$BINARY" start --no-open \
  >"$ROOT/different-config.out" 2>"$ROOT/different-config.err"
[[ "$(status_value PID "$ROOT/different-config.out")" == "$FIRST_PID" ]] ||
  fail "different requested configuration replaced the running instance"
grep -q '^warning: requested configuration was not applied' "$ROOT/different-config.err" ||
  fail "different requested configuration did not warn that the active config won"

"$BINARY" stop >"$ROOT/stop.out"
wait_for_exit "$FIRST_PID"
set +e
"$BINARY" status >"$ROOT/stopped-status.out" 2>&1
STOPPED_EXIT=$?
set -e
[[ "$STOPPED_EXIT" == 1 ]] ||
  fail "stopped status returned $STOPPED_EXIT instead of 1"
grep -q '^Yard is not running$' "$ROOT/stopped-status.out" ||
  fail "stopped status output was unclear"
"$BINARY" stop >"$ROOT/stop-again.out"
grep -q '^Yard is already stopped$' "$ROOT/stop-again.out" ||
  fail "repeated stop was not idempotent"

"$BINARY" start --no-open >"$ROOT/stale-start.out"
"$BINARY" status >"$ROOT/stale-status.out"
STALE_PID=$(status_value PID "$ROOT/stale-status.out")
kill -KILL "$STALE_PID"
wait_for_exit "$STALE_PID"
"$BINARY" start --no-open >"$ROOT/recovered-start.out"
RECOVERED_PID=$(status_value PID "$ROOT/recovered-start.out")
[[ -n "$RECOVERED_PID" && "$RECOVERED_PID" != "$STALE_PID" ]] ||
  fail "stale managed state was not recovered"
"$BINARY" stop >/dev/null

"$BINARY" start --no-open >"$ROOT/sigterm-start.out"
"$BINARY" status >"$ROOT/sigterm-status.out"
SIGTERM_PID=$(status_value PID "$ROOT/sigterm-status.out")
kill -TERM "$SIGTERM_PID"
wait_for_exit "$SIGTERM_PID"
for _ in {1..200}; do
  set +e
  "$BINARY" status >"$ROOT/sigterm-stopped.out" 2>&1
  SIGTERM_STATUS=$?
  set -e
  [[ "$SIGTERM_STATUS" == 1 ]] && break
  sleep 0.05
done
[[ "${SIGTERM_STATUS:-0}" == 1 ]] ||
  fail "managed SIGTERM did not release lifecycle ownership"
grep -q '^Yard is not running$' "$ROOT/sigterm-stopped.out" ||
  fail "managed SIGTERM left stale or unresponsive lifecycle state"

"$BINARY" start >"$ROOT/browser-success.out"
"$BINARY" status >"$ROOT/browser-success-status.out"
BROWSER_SUCCESS_URL=$(status_value URL "$ROOT/browser-success-status.out")
[[ "$(tail -n 1 "$YARD_BROWSER_MARKER")" == "$BROWSER_SUCCESS_URL" ]] ||
  fail "browser launcher did not receive the resolved Yard URL"
[[ "$(tail -n 1 "$YARD_BROWSER_ARGC_MARKER")" == 1 ]] ||
  fail "browser launcher did not receive exactly one argument"
curl -fsS "${BROWSER_SUCCESS_URL}health" >/dev/null ||
  fail "Yard was not healthy after successful browser launch"
"$BINARY" stop >/dev/null

export YARD_BROWSER_EXIT=7
"$BINARY" start >"$ROOT/browser-start.out" 2>"$ROOT/browser-start.err"
grep -q '^warning: could not open the default browser:' "$ROOT/browser-start.err" ||
  fail "browser failure did not produce a warning"
[[ -s "$YARD_BROWSER_MARKER" ]] ||
  fail "browser launcher was not exercised"
"$BINARY" status >"$ROOT/browser-status.out"
BROWSER_URL=$(status_value URL "$ROOT/browser-status.out")
curl -fsS "${BROWSER_URL}health" >/dev/null ||
  fail "browser failure tore down the managed server"
"$BINARY" stop >/dev/null
unset YARD_BROWSER_EXIT

mkdir -p "$ROOT/empty-path"
PATH="$ROOT/empty-path" \
  "$BINARY" start >"$ROOT/browser-missing.out" 2>"$ROOT/browser-missing.err"
grep -q "could not run $BROWSER_COMMAND" "$ROOT/browser-missing.err" ||
  fail "missing browser launcher did not produce a warning"
"$BINARY" status >/dev/null ||
  fail "missing browser launcher tore down Yard"
"$BINARY" stop >/dev/null

export YARD_BROWSER_SLEEP=10
"$BINARY" start >"$ROOT/browser-timeout.out" 2>"$ROOT/browser-timeout.err"
grep -q 'did not exit within 5 seconds and was terminated' "$ROOT/browser-timeout.err" ||
  fail "browser timeout did not produce a bounded warning"
BROWSER_TIMEOUT_PID=$(<"$YARD_BROWSER_PID")
if kill -0 "$BROWSER_TIMEOUT_PID" >/dev/null 2>&1; then
  fail "timed-out browser launcher remained alive"
fi
"$BINARY" status >/dev/null ||
  fail "browser timeout tore down Yard"
"$BINARY" stop >/dev/null
unset YARD_BROWSER_SLEEP

YARD_DATABASE_PATH="$ROOT/foreground.sqlite3" \
  "$BINARY" run >"$ROOT/foreground.log" 2>&1 &
FOREGROUND_PID=$!
FOREGROUND_URL=''
for _ in {1..200}; do
  if ! kill -0 "$FOREGROUND_PID" >/dev/null 2>&1; then
    fail "foreground Yard exited during startup"
  fi
  FOREGROUND_URL=$(sed -n \
    's/.*ui_url=\(http:\/\/127\.0\.0\.1:[0-9][0-9]*\/\).*/\1/p' \
    "$ROOT/foreground.log" | tail -n 1)
  [[ -n "$FOREGROUND_URL" ]] && break
  sleep 0.05
done
[[ -n "$FOREGROUND_URL" ]] || fail "foreground Yard did not report its URL"
YARD_DATABASE_PATH="$ROOT/foreground.sqlite3" \
  "$BINARY" status >"$ROOT/foreground-status.out"
grep -q '^  Mode: foreground$' "$ROOT/foreground-status.out" ||
  fail "status did not identify the foreground owner"
[[ "$(status_value PID "$ROOT/foreground-status.out")" == "$FOREGROUND_PID" ]] ||
  fail "foreground status reported the wrong PID"
YARD_DATABASE_PATH="$ROOT/foreground.sqlite3" \
  "$BINARY" start --no-open >"$ROOT/foreground-start.out"
[[ "$(status_value PID "$ROOT/foreground-start.out")" == "$FOREGROUND_PID" ]] ||
  fail "start launched another process over a foreground owner"
set +e
YARD_DATABASE_PATH="$ROOT/foreground.sqlite3" \
  "$BINARY" stop >"$ROOT/foreground-stop.out" 2>"$ROOT/foreground-stop.err"
FOREGROUND_STOP_EXIT=$?
set -e
[[ "$FOREGROUND_STOP_EXIT" == 1 ]] ||
  fail "yard stop did not refuse foreground ownership"
grep -q 'stop it in its owning terminal' "$ROOT/foreground-stop.err" ||
  fail "foreground stop refusal was not actionable"
kill -0 "$FOREGROUND_PID" >/dev/null 2>&1 ||
  fail "yard stop terminated a foreground instance"
curl -fsS "${FOREGROUND_URL}health" >/dev/null ||
  fail "foreground instance was not healthy after yard stop"

CONFLICT_BIND=${FOREGROUND_URL#http://}
CONFLICT_BIND=${CONFLICT_BIND%/}
set +e
YARD_BIND="$CONFLICT_BIND" \
YARD_DATABASE_PATH="$ROOT/conflict.sqlite3" \
YARD_RUNTIME_DIR="$ROOT/conflict-runtime" \
  "$BINARY" start --no-open >"$ROOT/conflict.out" 2>"$ROOT/conflict.err"
CONFLICT_EXIT=$?
set -e
[[ "$CONFLICT_EXIT" != 0 ]] ||
  fail "managed start succeeded despite a bind conflict"
grep -qi 'address already in use' "$ROOT/conflict.err" ||
  fail "bind conflict did not produce a clear startup failure"

kill -TERM "$FOREGROUND_PID"
wait "$FOREGROUND_PID"
FOREGROUND_PID=''

sleep 60 &
UNRELATED_PID=$!
printf '%s\n' \
  "{\"protocol\":1,\"instance_id\":\"018f0000-0000-7000-8000-000000000001\",\"control_secret\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"config_fingerprint\":\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\",\"mode\":\"managed\",\"pid\":$UNRELATED_PID,\"address\":\"127.0.0.1:4317\",\"url\":\"http://127.0.0.1:4317/\",\"started_at\":\"2026-08-20T00:00:00Z\"}" \
  >"$RUNTIME_ROOT/instance.json"
chmod 600 "$RUNTIME_ROOT/instance.json"
"$BINARY" stop >"$ROOT/unrelated-stop.out"
kill -0 "$UNRELATED_PID" >/dev/null 2>&1 ||
  fail "yard stop signalled an unrelated PID from stale metadata"
kill "$UNRELATED_PID"
wait "$UNRELATED_PID" 2>/dev/null || true
UNRELATED_PID=''

CONCURRENT_STARTS=()
for index in {1..10}; do
  "$BINARY" start --no-open >"$ROOT/concurrent-$index.out" &
  CONCURRENT_STARTS+=("$!")
done
for pid in "${CONCURRENT_STARTS[@]}"; do
  wait "$pid"
done
CONCURRENT_PID=$(status_value PID "$ROOT/concurrent-1.out")
[[ -n "$CONCURRENT_PID" ]] ||
  fail "concurrent starts omitted process identity"
for index in {2..10}; do
  [[ "$(status_value PID "$ROOT/concurrent-$index.out")" == "$CONCURRENT_PID" ]] ||
    fail "ten concurrent starts did not converge on one managed process"
done
"$BINARY" stop >/dev/null

"$BINARY" start --no-open >"$ROOT/race-initial.out"
"$BINARY" stop >"$ROOT/race-stop.out" &
RACE_STOP=$!
"$BINARY" start --no-open >"$ROOT/race-start.out" &
RACE_START=$!
wait "$RACE_STOP"
wait "$RACE_START"
set +e
"$BINARY" status >"$ROOT/race-status.out" 2>&1
RACE_STATUS=$?
set -e
if [[ "$RACE_STATUS" == 0 ]]; then
  RACE_URL=$(status_value URL "$ROOT/race-status.out")
  curl -fsS "${RACE_URL}health" >/dev/null ||
    fail "start-stop race left an unhealthy lifecycle owner"
  "$BINARY" stop >/dev/null
elif [[ "$RACE_STATUS" != 1 ]]; then
  fail "start-stop race left an invalid lifecycle status: $RACE_STATUS"
fi

if node -e '
  const server = require("node:net").createServer();
  server.on("error", () => process.exit(1));
  server.listen(0, "::1", () => server.close(() => process.exit(0)));
'; then
  mkdir -p "$ROOT/ipv6-runtime"
  chmod 700 "$ROOT/ipv6-runtime"
  YARD_BIND='[::1]:0' \
  YARD_DATABASE_PATH="$ROOT/ipv6.sqlite3" \
  YARD_RUNTIME_DIR="$ROOT/ipv6-runtime" \
    "$BINARY" start --no-open >"$ROOT/ipv6-start.out"
  IPV6_URL=$(status_value URL "$ROOT/ipv6-start.out")
  [[ "$IPV6_URL" =~ ^http://\[::1\]:[1-9][0-9]*/$ ]] ||
    fail "IPv6 port-zero URL was not correctly bracketed: $IPV6_URL"
  curl --globoff -fsS "${IPV6_URL}health" >/dev/null ||
    fail "IPv6 port-zero URL was not reachable"
  YARD_BIND='[::1]:0' \
  YARD_DATABASE_PATH="$ROOT/ipv6.sqlite3" \
  YARD_RUNTIME_DIR="$ROOT/ipv6-runtime" \
    "$BINARY" stop >/dev/null
else
  printf 'Yard CLI lifecycle smoke: IPv6 loopback unavailable; skipped\n' >&2
fi

mkdir -p "$SECOND_HOME" "$SECOND_DATA" "$SECOND_STATE" "$SECOND_RUNTIME"
chmod 700 "$SECOND_RUNTIME"
"$BINARY" start --no-open >"$ROOT/isolated-first.out"
HOME="$SECOND_HOME" \
XDG_DATA_HOME="$SECOND_DATA" \
XDG_STATE_HOME="$SECOND_STATE" \
XDG_RUNTIME_DIR="$SECOND_RUNTIME" \
YARD_ORCHESTRATOR_CWD="$ROOT" \
  "$BINARY" start --no-open >"$ROOT/isolated-second.out"
HOME="$SECOND_HOME" \
XDG_DATA_HOME="$SECOND_DATA" \
XDG_STATE_HOME="$SECOND_STATE" \
XDG_RUNTIME_DIR="$SECOND_RUNTIME" \
  "$BINARY" status >"$ROOT/isolated-second-status.out"
ISOLATED_FIRST_PID=$(status_value PID "$ROOT/isolated-first.out")
ISOLATED_SECOND_PID=$(status_value PID "$ROOT/isolated-second-status.out")
[[ -n "$ISOLATED_FIRST_PID" && -n "$ISOLATED_SECOND_PID" ]] ||
  fail "isolated instances omitted process identity"
[[ "$ISOLATED_FIRST_PID" != "$ISOLATED_SECOND_PID" ]] ||
  fail "isolated database roots crossed lifecycle ownership"
HOME="$SECOND_HOME" \
XDG_DATA_HOME="$SECOND_DATA" \
XDG_STATE_HOME="$SECOND_STATE" \
XDG_RUNTIME_DIR="$SECOND_RUNTIME" \
  "$BINARY" stop >/dev/null
"$BINARY" status >"$ROOT/isolated-first-status.out"
[[ "$(status_value PID "$ROOT/isolated-first-status.out")" == "$ISOLATED_FIRST_PID" ]] ||
  fail "stopping the second instance affected the first"
"$BINARY" stop >/dev/null

printf 'Yard CLI lifecycle smoke passed\n'
printf '  managed URL: %s\n' "$FIRST_URL"
printf '  stopped status exit: %s\n' "$STOPPED_EXIT"
printf '  stale PID: %s, recovered PID: %s\n' "$STALE_PID" "$RECOVERED_PID"

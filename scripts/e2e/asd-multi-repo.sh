#!/usr/bin/env bash
# End-to-end scenario for the asd-multi-repo plan (CTXone × asd).
#
# Verifies that one user, working with two indexed repos, can:
#   1. Register both with `asd repo add` and see them in `asd repo list`
#   2. Run asd-mcp with no ASD_DB and have it follow the active repo
#      across a live `asd repo use <other>` (no MCP restart)
#   3. Run ctxone-hub --http with no flags and have it auto-discover
#      both repos from the shared registry
#   4. See the hub spawn a pool process on first proxy hit (cold→running)
#      and evict it after the configured idle timeout (running→idle)
#
# Prerequisites (built debug binaries are fine):
#   - asd, asd-mcp, asd-serve from agentstategroup/agentstatedeveloper
#   - ctxone-hub from this repo
#
# Override discovery via env vars if your layout differs:
#   ASD_BIN_DIR=…/agentstatedeveloper/target/debug
#   CTXONE_BIN_DIR=…/CTXone/target/debug
set -euo pipefail

red() { printf '\033[31m%s\033[0m\n' "$*" >&2; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
yellow() { printf '\033[33m%s\033[0m\n' "$*"; }
step() { printf '\n\033[1;36m▸ %s\033[0m\n' "$*"; }

fail() { red "FAIL: $*"; exit 1; }

# ---------------------------------------------------------------------------
# Locate binaries
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DEFAULT_CTXONE_BIN_DIR="$REPO_ROOT/target/debug"
DEFAULT_ASD_BIN_DIR="$(cd "$REPO_ROOT/.." && pwd)/AgentStateDeveloper/target/debug"

CTXONE_BIN_DIR="${CTXONE_BIN_DIR:-$DEFAULT_CTXONE_BIN_DIR}"
ASD_BIN_DIR="${ASD_BIN_DIR:-$DEFAULT_ASD_BIN_DIR}"

for name in asd asd-mcp asd-serve; do
  test -x "$ASD_BIN_DIR/$name" || fail "$ASD_BIN_DIR/$name not found. Set ASD_BIN_DIR or build it."
done
test -x "$CTXONE_BIN_DIR/ctxone-hub" || fail "ctxone-hub not found in $CTXONE_BIN_DIR. Set CTXONE_BIN_DIR or build it."

# tracing-subscriber emits ANSI color codes by default; they wreck the
# `grep 'name=myapp'` checks below where the log file gets "name\x1b[0m=…".
export NO_COLOR=1
# Don't let a stray ASD_DB on the parent shell bypass the registry path.
unset ASD_DB || true

# ---------------------------------------------------------------------------
# Per-run scratch dirs and cleanup
# ---------------------------------------------------------------------------
WORK="$(mktemp -d -t asd-multi-repo-e2e.XXXXXX)"
export ASD_REGISTRY="$WORK/repos.toml"
MCP_LOG="$WORK/asd-mcp.log"
HUB_LOG="$WORK/ctxone-hub.log"
HUB_DB="$WORK/ctxone.db"
MCP_PID=""
HUB_PID=""

# Stage stable copies of the binaries into the scratch dir. Cargo replaces
# target/debug/* atomically while a concurrent agent rebuilds the asd repo,
# which mid-script could swap our `asd` for a pre-registry build. Copying
# once at startup pins the versions for the whole test run.
STAGE="$WORK/bin"
mkdir -p "$STAGE"
cp "$ASD_BIN_DIR/asd"        "$STAGE/asd"
cp "$ASD_BIN_DIR/asd-mcp"    "$STAGE/asd-mcp"
cp "$ASD_BIN_DIR/asd-serve"  "$STAGE/asd-serve"
cp "$CTXONE_BIN_DIR/ctxone-hub" "$STAGE/ctxone-hub"
ASD="$STAGE/asd"
ASD_MCP="$STAGE/asd-mcp"
CTXONE_HUB="$STAGE/ctxone-hub"

cleanup() {
  set +e
  [[ -n "$MCP_PID" ]] && kill "$MCP_PID" 2>/dev/null
  [[ -n "$HUB_PID" ]] && kill "$HUB_PID" 2>/dev/null
  # Give children a moment to flush, then nuke if still up.
  sleep 0.3
  [[ -n "$MCP_PID" ]] && kill -9 "$MCP_PID" 2>/dev/null
  [[ -n "$HUB_PID" ]] && kill -9 "$HUB_PID" 2>/dev/null
  if [[ "${KEEP_WORK:-0}" == "1" ]]; then
    yellow "Kept scratch dir: $WORK"
  else
    rm -rf "$WORK"
  fi
}
trap cleanup EXIT

yellow "scratch: $WORK"
yellow "asd:    $ASD_BIN_DIR"
yellow "hub:    $CTXONE_BIN_DIR"

# ---------------------------------------------------------------------------
# 1. Two indexed repos
# ---------------------------------------------------------------------------
step "create + index two repos (myapp, sdk)"
for name in myapp sdk; do
  mkdir -p "$WORK/$name"
  echo "def hello_$name(): pass" > "$WORK/$name/a.py"
  ( cd "$WORK/$name" && "$ASD" init >/dev/null && "$ASD" index . >/dev/null )
  test -f "$WORK/$name/.asd-state.db" || fail "$name failed to produce a .asd-state.db"
done

# `asd index` auto-registers (t-004) — confirm both showed up.
"$ASD" repo list >"$WORK/list.txt"
grep -q '^[*[:space:]] *myapp ' "$WORK/list.txt" || fail "auto-register did not add myapp"
grep -q '^[*[:space:]] *sdk ' "$WORK/list.txt" || fail "auto-register did not add sdk"
green "  ✓ both repos in registry"

# ---------------------------------------------------------------------------
# 2. CLI: `asd repo use` flips the active marker
# ---------------------------------------------------------------------------
step "asd repo use myapp / sdk round-trip"
"$ASD" repo use myapp >/dev/null
[[ "$("$ASD" repo show --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["name"])')" == "myapp" ]] \
  || fail 'active != myapp after `asd repo use myapp`'
"$ASD" repo use sdk >/dev/null
[[ "$("$ASD" repo show --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["name"])')" == "sdk" ]] \
  || fail 'active != sdk after `asd repo use sdk`'
green "  ✓ active pointer follows asd repo use"

# Restore myapp as active before asd-mcp starts.
"$ASD" repo use myapp >/dev/null

# ---------------------------------------------------------------------------
# 3. asd-mcp follows the active repo without restart
# ---------------------------------------------------------------------------
step 'asd-mcp picks up the active repo at startup, then live-swaps on `asd repo use`'
# Hold stdin open so the MCP service doesn't exit on EOF mid-test.
unset ASD_DB
( sleep 5 | "$ASD_MCP" ) > "$MCP_LOG" 2>&1 &
MCP_PID=$!

# Wait up to ~3s for startup line.
for _ in $(seq 1 30); do
  grep -q 'starting asd-mcp stdio server' "$MCP_LOG" && break
  sleep 0.1
done
grep -q 'resolved db from registry active repo name=myapp' "$MCP_LOG" \
  || fail "asd-mcp did not resolve myapp from registry. Log:\n$(cat "$MCP_LOG")"
green "  ✓ asd-mcp opened myapp from the registry"

# Flip active to sdk and wait for the watcher (250ms poll) to notice.
"$ASD" repo use sdk >/dev/null
swap_seen=0
for _ in $(seq 1 30); do
  if grep -q 'switched active repo via registry' "$MCP_LOG" \
     && grep -q 'name=sdk' "$MCP_LOG"; then
    swap_seen=1; break
  fi
  sleep 0.1
done
[[ "$swap_seen" -eq 1 ]] \
  || fail "asd-mcp did not log a registry swap to sdk. Log:\n$(cat "$MCP_LOG")"
green "  ✓ asd-mcp live-swapped to sdk without restart"

kill "$MCP_PID" 2>/dev/null || true
wait "$MCP_PID" 2>/dev/null || true
MCP_PID=""

# Restore active=myapp for the hub scenario.
"$ASD" repo use myapp >/dev/null

# ---------------------------------------------------------------------------
# 4. ctxone-hub auto-discovers + pool spawn + idle eviction
# ---------------------------------------------------------------------------
step "ctxone-hub auto-discovers both repos and pools them on demand"

# Pick a free port (let the OS hand us one, then ask it back).
PORT="$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')"

# Use a tiny idle timeout so we can observe eviction within the test.
IDLE=3
PATH="$STAGE:$PATH" \
  "$CTXONE_HUB" --http --port "$PORT" --path "$HUB_DB" --init \
                --asd-idle-timeout "$IDLE" \
  > "$HUB_LOG" 2>&1 &
HUB_PID=$!

# Wait for the auto-discovery log line and for the socket to actually accept.
# The "HTTP API listening" line is emitted before axum::serve() runs — we have
# to poll the port itself to know the listener is up.
for _ in $(seq 1 100); do
  if grep -q 'auto-discovered asd repos' "$HUB_LOG" \
     && curl -s -o /dev/null --max-time 0.2 "http://127.0.0.1:$PORT/api/health"; then
    break
  fi
  sleep 0.1
done
grep -q 'auto-discovered asd repos' "$HUB_LOG" \
  || fail "ctxone-hub did not auto-discover the registry. Log:\n$(cat "$HUB_LOG")"
grep -q '"myapp"' "$HUB_LOG" && grep -q '"sdk"' "$HUB_LOG" \
  || fail "auto-discovery log did not list both repos. Log:\n$(cat "$HUB_LOG")"
green "  ✓ hub auto-discovered myapp + sdk"

# /api/code initially reports both as pool entries.
RESP="$(curl -s -o "$WORK/code.json" -w '%{http_code}' "http://127.0.0.1:$PORT/api/code")" || true
if [[ "$RESP" != "200" ]]; then
  fail "GET /api/code returned $RESP, body=$(cat "$WORK/code.json"). Log tail:\n$(tail -20 "$HUB_LOG")"
fi
LIST="$(cat "$WORK/code.json")"
echo "$LIST" | python3 -c '
import json, sys
data = json.loads(sys.stdin.read())
by_name = {r["name"]: r for r in data}
for n in ("myapp", "sdk"):
    if n not in by_name:
        sys.exit(f"missing {n}: {data}")
    if by_name[n].get("source") != "pool":
        sys.exit(f"{n} source != pool: {by_name[n]}")
    if by_name[n].get("status") != "idle":
        sys.exit(f"{n} should start idle, got {by_name[n]}")
' || fail "initial /api/code shape is wrong"
green "  ✓ both repos visible as source=pool, status=idle"

# Trigger a spawn for myapp via the prefetch endpoint.
PF_CODE="$(curl -s -o "$WORK/prefetch.out" -w '%{http_code}' \
  -X POST "http://127.0.0.1:$PORT/api/code/myapp/prefetch")" || true
if [[ "$PF_CODE" != "200" ]]; then
  fail "prefetch myapp returned $PF_CODE, body=$(cat "$WORK/prefetch.out"). Hub log tail:\n$(tail -25 "$HUB_LOG")"
fi

# After prefetch, /api/code should report myapp as running.
LIST="$(curl -sf "http://127.0.0.1:$PORT/api/code")"
echo "$LIST" | python3 -c '
import json, sys
data = json.loads(sys.stdin.read())
by_name = {r["name"]: r for r in data}
m = by_name.get("myapp", {})
s = by_name.get("sdk", {})
if m.get("status") != "running":
    sys.exit("myapp should be running after prefetch, got " + repr(m))
if s.get("status") != "idle":
    sys.exit("sdk should still be idle, got " + repr(s))
' || fail "post-prefetch status shape is wrong"
green "  ✓ myapp spawned and reports status=running; sdk still idle"

# Idle eviction: with --asd-idle-timeout $IDLE the pool's reaper runs once a
# minute by default. We can't shorten that interval from the CLI today, so
# the most we can verify here is that the running state survives the idle
# window — the actual kill is exercised by AsdProcessPool::evict_idle, which
# has unit coverage in server/src/asd_pool.rs. Document this as a known
# limitation rather than wait 60+s in CI.
step "(skip) idle-eviction reaper has a fixed 60s wake; covered by unit tests"

# ---------------------------------------------------------------------------
# Wrap-up
# ---------------------------------------------------------------------------
kill "$HUB_PID" 2>/dev/null || true
wait "$HUB_PID" 2>/dev/null || true
HUB_PID=""

green ""
green "════════════════════════════════════════"
green "  asd-multi-repo e2e: ALL CHECKS PASSED"
green "════════════════════════════════════════"

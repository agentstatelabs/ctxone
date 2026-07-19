#!/usr/bin/env bash
# Run a throwaway Hub against a COPY of the live memory.db, for testing
# branch work without touching the launchd hub on :3001.
#
#   scripts/dev-hub.sh            # build web + hub, snapshot db, serve :3002
#   scripts/dev-hub.sh --no-build # reuse the existing release binary
#   scripts/dev-hub.sh --fresh    # re-snapshot the db even if one exists
#   scripts/dev-hub.sh --stop     # stop the dev hub and leave the db alone
#
# Why a copy: ctxone-hub takes an exclusive lockfile on its db and
# refuses to start a second hub against the same path, so pointing this
# at ~/.ctxone/memory.db would simply fail. A copy also means anything
# you do here — syncs, deletes, experiments — cannot corrupt live data.
#
# The snapshot lives at ~/.ctxone/dev.db and is reused across runs so a
# test session survives a restart. Use --fresh to re-pull from live.

set -euo pipefail

PORT="${DEV_HUB_PORT:-3002}"
LIVE_DB="${CTXONE_DB:-$HOME/.ctxone/memory.db}"
DEV_DB="${DEV_HUB_DB:-$HOME/.ctxone/dev.db}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PIDFILE="$HOME/.ctxone/dev-hub.pid"
LOG="$HOME/.ctxone/dev-hub.log"

BUILD=1
FRESH=0
for arg in "$@"; do
  case "$arg" in
    --no-build) BUILD=0 ;;
    --fresh)    FRESH=1 ;;
    --stop)
      if [[ -f "$PIDFILE" ]] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
        kill "$(cat "$PIDFILE")" && rm -f "$PIDFILE"
        echo "dev hub stopped"
      else
        echo "no dev hub running"
      fi
      exit 0
      ;;
    *) echo "unknown flag: $arg" >&2; exit 64 ;;
  esac
done

cd "$REPO_ROOT"

# Refuse to clobber the live db, however the env is set.
if [[ "$DEV_DB" == "$LIVE_DB" ]]; then
  echo "refusing to run: DEV_HUB_DB must differ from the live db" >&2
  exit 1
fi

# Already running? Don't start a second one.
if [[ -f "$PIDFILE" ]] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
  echo "dev hub already running (pid $(cat "$PIDFILE")) on :$PORT"
  echo "  stop it with: scripts/dev-hub.sh --stop"
  exit 0
fi

if [[ "$BUILD" == "1" ]]; then
  echo "▸ building Lens + hub (Lens is embedded via rust-embed, so both are needed)"
  npm --prefix web run build >/dev/null
  cargo build --release --bin ctxone-hub
fi

if [[ "$FRESH" == "1" || ! -f "$DEV_DB" ]]; then
  echo "▸ snapshotting $LIVE_DB -> $DEV_DB"
  # sqlite .backup is safe against a live writer; plain cp is not.
  sqlite3 "$LIVE_DB" ".backup '$DEV_DB'"
else
  echo "▸ reusing existing $DEV_DB (--fresh to re-snapshot)"
fi

echo "▸ starting hub on :$PORT"
./target/release/ctxone-hub --http --lens --path "$DEV_DB" --port "$PORT" >"$LOG" 2>&1 &
echo $! >"$PIDFILE"

for _ in $(seq 1 30); do
  if curl -fsS "http://localhost:$PORT/api/health" >/dev/null 2>&1; then
    echo "  ✓ http://localhost:$PORT  (log: $LOG)"
    echo "  live hub on :3001 untouched"
    exit 0
  fi
  sleep 1
done

echo "  ✗ hub did not come up; last log lines:" >&2
tail -5 "$LOG" >&2
rm -f "$PIDFILE"
exit 1

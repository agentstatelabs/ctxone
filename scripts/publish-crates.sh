#!/usr/bin/env bash
#
# publish-crates.sh — publish ctxone and ctxone-hub to crates.io.
#
# Usage: scripts/publish-crates.sh             # dry run (any clean checkout)
#        scripts/publish-crates.sh --publish   # real upload (release tag only)
#
# ctxone-hub embeds the Lens build, which scripts/stage-lens.sh copies into
# server/lens/. That directory is gitignored, so cargo counts the staged files
# as uncommitted changes and refuses to package them without --allow-dirty.
# This script runs cargo with --allow-dirty only after proving that nothing
# else is uncommitted, so the flag covers the staged Lens and nothing more.
#
# A crates.io version can be yanked but never deleted or re-uploaded, so
# --publish insists on a clean, detached checkout of the release tag whose
# version matches the workspace.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

MODE="--dry-run"
case "${1:-}" in
  "") ;;
  --publish) MODE="" ;;
  *) echo "usage: $0 [--publish]" >&2; exit 2 ;;
esac

if [ -n "$(git status --porcelain)" ]; then
  echo "error: uncommitted changes; publish only committed code:" >&2
  git status --short >&2
  exit 1
fi

VER="$(awk '/^\[workspace\.package\]/{f=1;next} f&&/^version = /{gsub(/[",]/,"",$3);print $3;exit}' Cargo.toml)"
TAG="$(git describe --tags --exact-match HEAD 2>/dev/null || true)"
if [ -z "$MODE" ]; then
  [ "$TAG" = "v$VER" ] || {
    echo "error: --publish needs HEAD at tag v$VER (HEAD is ${TAG:-untagged})" >&2
    exit 1
  }
elif [ "$TAG" != "v$VER" ]; then
  echo "note: HEAD is not tag v$VER; fine for a dry run, not for --publish"
fi

scripts/stage-lens.sh

# Everything but the gitignored server/lens/ was checked clean above.
cargo publish --workspace --allow-dirty $MODE

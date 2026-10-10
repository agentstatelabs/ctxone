#!/usr/bin/env bash
#
# publish-crates.sh — publish ctxone and ctxone-hub to crates.io.
#
# Usage: scripts/publish-crates.sh             # dry run (any clean checkout)
#        scripts/publish-crates.sh --publish   # real upload (release tag only)
#
# The publish-crates CI job runs this on a release tag, with SKIP_BUILD=1 and
# the frontend job's web/build/. RELEASE.md covers running it by hand from a
# detached checkout of the tag. cargo takes the crates.io token from
# CARGO_REGISTRY_TOKEN (CI) or `cargo login` (by hand).
#
# ctxone-hub embeds the Lens build, which scripts/stage-lens.sh copies into
# server/lens/. That directory is gitignored, so cargo counts the staged files
# as uncommitted changes and refuses to package them without --allow-dirty.
# This script runs cargo with --allow-dirty only after proving that nothing
# else is uncommitted, so the flag covers the staged Lens and nothing more.
#
# Safe to re-run: a crate already on crates.io at this version is skipped. A
# crates.io version can be yanked but never deleted or re-uploaded, so
# --publish insists on committed code at the release tag whose version matches
# the workspace.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

MODE="--dry-run"
case "${1:-}" in
  "") ;;
  --publish) MODE="" ;;
  *) echo "usage: $0 [--publish]" >&2; exit 2 ;;
esac

# What --allow-dirty would otherwise let through: a change to any tracked file,
# or an untracked file inside a crate, which cargo would package. Untracked
# files elsewhere, such as CI's cargo home and caches, never reach a package.
DIRTY="$(git status --porcelain --untracked-files=no
         git status --porcelain --untracked-files=all -- cli server | grep '^??' || true)"
if [ -n "$DIRTY" ]; then
  echo "error: uncommitted changes; publish only committed code:" >&2
  printf '%s\n' "$DIRTY" >&2
  exit 1
fi

VER="$(awk '/^\[workspace\.package\]/{f=1;next} f&&/^version = /{gsub(/[",]/,"",$3);print $3;exit}' Cargo.toml)"
TAG="$(git describe --tags --exact-match HEAD 2>/dev/null || true)"
# A tag pipeline's checkout may lack the tag ref; GitLab names it for us.
if [ -z "$TAG" ] && [ -n "${CI_COMMIT_TAG:-}" ] && [ "$(git rev-parse HEAD)" = "${CI_COMMIT_SHA:-}" ]; then
  TAG="$CI_COMMIT_TAG"
fi
if [ -z "$MODE" ]; then
  [ "$TAG" = "v$VER" ] || {
    echo "error: --publish needs HEAD at tag v$VER (HEAD is ${TAG:-untagged})" >&2
    exit 1
  }
elif [ "$TAG" != "v$VER" ]; then
  echo "note: HEAD is not tag v$VER; fine for a dry run, not for --publish"
fi

EXCLUDE=()
TODO=0
for c in ctxone ctxone-hub; do
  code="$(curl -s -o /dev/null -w '%{http_code}' -A "agentstatelabs-publish-crates" \
    "https://crates.io/api/v1/crates/$c/$VER")"
  case "$code" in
    200) echo "skip $c $VER (already on crates.io)"; EXCLUDE+=(--exclude "$c") ;;
    404) TODO=$((TODO + 1)) ;;
    *) echo "error: crates.io answered $code for $c $VER; not publishing" >&2; exit 1 ;;
  esac
done
if [ "$TODO" -eq 0 ]; then
  echo "both crates are already on crates.io at $VER; nothing to do"
  exit 0
fi

scripts/stage-lens.sh

# Everything but the gitignored server/lens/ was checked clean above.
cargo publish --workspace --locked --allow-dirty ${EXCLUDE[@]+"${EXCLUDE[@]}"} $MODE

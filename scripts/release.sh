#!/usr/bin/env bash
# Cut a release of CTXone by bumping the version, tagging, and pushing.
#
#   scripts/release.sh v0.9.37
#
# This script BUILDS NOTHING. All five platform builds, the GitHub release,
# and the Homebrew formula are produced by .github/workflows/release.yml on
# GitHub-hosted runners (macos-14 / ubuntu-22.04 / windows-latest). Pushing
# the tag is the entire trigger.
#
# Why it no longer builds locally:
#   This script used to cross-compile via cross-rs + Docker and publish the
#   tarballs itself, duplicating what CI already did on every tag. Two
#   publishers racing on one release is how you get assets whose sha256s
#   disagree with what CI built (see the asd v0.9.38 incident). There is now
#   exactly one publisher.
#
# What it does:
#   1. Refuse if the working tree is dirty or the branch is behind origin.
#   2. Bump the workspace version across Cargo.toml, the Python bindings and
#      both frontends, cargo check, and commit — kept here because the
#      version-guard CI job requires these to move in lockstep.
#   3. Tag $VERSION on HEAD (annotated) if not already there.
#   4. Push main + the tag to GitLab origin. GitLab CI mirrors them to
#      GitHub through the leak-scan gate, which fires the release workflow.
#   5. Print the Actions run to watch.
#
# Env overrides:
#   SKIP_SYNC_CHECK=1 — don't require being level with origin/main
#   SKIP_BUMP=1       — tag HEAD as-is without touching version files

set -euo pipefail

VERSION="${1:-}"
if [[ -z "$VERSION" || ! "$VERSION" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "usage: $0 vMAJOR.MINOR.PATCH"
  echo "  example: $0 v0.9.37"
  exit 64
fi
VER_NUM="${VERSION#v}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ACTIONS_REPO="agentstatelabs/ctxone"

step()  { printf '\n\033[1;36m▸ %s\033[0m\n' "$*"; }
ok()    { printf '\033[32m  ✓ %s\033[0m\n' "$*"; }
warn()  { printf '\033[33m  ⚠ %s\033[0m\n' "$*"; }
fail()  { printf '\033[31m  ✗ %s\033[0m\n' "$*" >&2; exit 1; }

cd "$REPO_ROOT"

# ---------------------------------------------------------------------------
# 1. Preflight
# ---------------------------------------------------------------------------
step "preflight"

[[ -z "$(git status --porcelain)" ]] || fail "working tree dirty — commit or stash first"
ok "working tree clean"

# Never release from a stale local main: that either fails on a non-fast-forward
# push or quietly reverts upstream commits.
if [[ "${SKIP_SYNC_CHECK:-0}" != "1" ]]; then
  if git fetch --quiet origin main 2>/dev/null; then
    behind="$(git rev-list --count HEAD..origin/main 2>/dev/null || echo 0)"
    [[ "$behind" == "0" ]] || \
      fail "branch is $behind commit(s) behind origin/main — pull first (or SKIP_SYNC_CHECK=1)"
    ok "level with origin/main"
  else
    warn "could not fetch origin (offline?) — skipping sync check"
  fi
fi

# ---------------------------------------------------------------------------
# 2. Version bump (idempotent)
# ---------------------------------------------------------------------------
if [[ "${SKIP_BUMP:-0}" != "1" ]]; then
  step "bump workspace version to $VER_NUM"
  CUR="$(awk '/^\[workspace\.package\]/{f=1;next} f&&/^version = /{gsub(/[",]/,"",$3);print $3;exit}' Cargo.toml)"
  if [[ "$CUR" != "$VER_NUM" ]]; then
    perl -i -pe 'if(!$d && s/^version = "[^"]*"/version = "'"$VER_NUM"'"/){$d=1}' Cargo.toml
    # Sibling components move in lockstep with the product version — the
    # version-guard CI job fails the pipeline if they drift.
    perl -i -pe 'if(!$d && s/^version = "[^"]*"/version = "'"$VER_NUM"'"/){$d=1}' bindings/python/pyproject.toml
    perl -i -pe 'if(!$d && s/("version":\s*)"[^"]*"/${1}"'"$VER_NUM"'"/){$d=1}' web/package.json
    perl -i -pe 'if(!$d && s/("version":\s*)"[^"]*"/${1}"'"$VER_NUM"'"/){$d=1}' website/package.json
    cargo check --workspace --release >/dev/null 2>&1 || \
      cargo check --workspace --release   # surface the real error if it fails
    git add Cargo.toml Cargo.lock bindings/python/pyproject.toml \
            web/package.json website/package.json
    git commit -m "release: $VERSION" >/dev/null
    ok "bumped $CUR → $VER_NUM (Cargo, pyproject, web, website) and committed"
  else
    ok "already at $VER_NUM"
  fi
else
  ok "SKIP_BUMP=1 — tagging HEAD as-is"
fi

# ---------------------------------------------------------------------------
# 3. Tag
# ---------------------------------------------------------------------------
step "tag $VERSION on HEAD"

HEAD_SHA="$(git rev-parse HEAD)"
if ! git rev-parse "$VERSION" >/dev/null 2>&1; then
  git tag -a "$VERSION" -m "$VERSION"
  ok "tagged $VERSION at $(git rev-parse --short HEAD)"
else
  # ^{commit} is required: the tag above is ANNOTATED, so a bare rev-parse
  # yields the tag object and never equals HEAD's commit.
  EXISTING="$(git rev-parse "$VERSION^{commit}")"
  [[ "$EXISTING" == "$HEAD_SHA" ]] || \
    fail "tag $VERSION already points at $EXISTING, but HEAD is $HEAD_SHA"
  warn "tag $VERSION already on HEAD — reusing"
fi

# ---------------------------------------------------------------------------
# 4. Push to GitLab only.
#
# Do NOT push to GitHub from here. GitLab's publish-github job is fail-closed
# on scripts/leak-scan.sh, and a direct push from a workstation bypasses that
# gate entirely — main and the tag reach the public mirror unscanned. GitLab
# CI mirrors them itself, which is what fires the GitHub release workflow.
# ---------------------------------------------------------------------------
step "push main + $VERSION"
git push origin main "$VERSION" 2>&1 | tail -3 | sed 's/^/  /'
ok "pushed to GitLab origin — CI will mirror main + tag to GitHub"

step "done — CI is building $VERSION"
echo "  watch:    gh run watch -R $ACTIONS_REPO"
echo "  runs:     https://github.com/$ACTIONS_REPO/actions"
echo "  release:  https://github.com/agentstatelabs/ctxone-releases/releases/tag/$VERSION"
echo
echo "  CI publishes the tarballs and pushes the Homebrew formula."
echo "  Nothing further to run locally."

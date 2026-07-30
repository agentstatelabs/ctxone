#!/usr/bin/env bash
# Cut a new local release of CTXone end-to-end.
#
#   scripts/release.sh v0.9.12
#
# What it does:
#   1. Bump workspace Cargo.toml to the supplied version, commit, tag, push
#      to GitLab origin.
#   2. cargo build --release for four targets:
#        aarch64-apple-darwin   (native)
#        x86_64-apple-darwin    (cargo --target)
#        x86_64-unknown-linux-gnu  (via cross-rs, needs Docker running)
#        aarch64-unknown-linux-gnu (via cross-rs, needs Docker running)
#   3. Tarball each as ctxone-<ver>-<target>.tar.gz containing ctxone-hub
#      and ctx flat under one top-level dir.
#   4. gh release create on agentstatelabs/ctxone-releases (or upload to
#      the existing release if the tag is already there) and attach all
#      four tarballs.
#   5. Patch the four URL + sha256 pairs in
#      Apps/homebrew-ctxone/Formula/ctxone.rb, commit + push to GitLab.
#      The GitLab → GitHub push mirror replicates to
#      agentstatelabs/homebrew-ctxone within seconds.
#
# Prereqs (one-time):
#   - rustup target add x86_64-apple-darwin
#   - cargo install cross --git https://github.com/cross-rs/cross
#   - Docker Desktop installed (must be running when cross builds Linux)
#   - gh authenticated as agentstatelabs (with at least Contents:write on
#     ctxone-releases) AND as ctxone (or any owner of the formula push).
#     The script switches between them automatically.
#   - Apps/homebrew-ctxone sibling clone exists with origin = GitLab.
#
# Env overrides for partial runs:
#   SKIP_TAG=1     — don't bump Cargo + tag (use when re-running for a
#                    version that's already tagged)
#   SKIP_LINUX=1   — only macOS targets
#   SKIP_FORMULA=1 — leave the brew tap alone
#   ONLY_TARGETS="a,b" — comma-separated subset to build
#                       (aarch64-apple-darwin, x86_64-apple-darwin,
#                        x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu)

set -euo pipefail

VERSION="${1:-}"
if [[ -z "$VERSION" || ! "$VERSION" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "usage: $0 vMAJOR.MINOR.PATCH"
  echo "  example: $0 v0.9.12"
  exit 64
fi
VER_NUM="${VERSION#v}"

# ---------------------------------------------------------------------------
# Paths and helpers
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TAP_ROOT="$(cd "$REPO_ROOT/../homebrew-ctxone" && pwd)"
RELEASE_REPO="agentstatelabs/ctxone-releases"
TAP_GITLAB="ssh://git@git.internal.example:2222/agentstategroup/homebrew-ctxone.git"

ALL_TARGETS=(
  aarch64-apple-darwin
  x86_64-apple-darwin
  x86_64-unknown-linux-gnu
  aarch64-unknown-linux-gnu
)

if [[ -n "${ONLY_TARGETS:-}" ]]; then
  IFS=',' read -ra TARGETS <<<"$ONLY_TARGETS"
elif [[ "${SKIP_LINUX:-0}" == "1" ]]; then
  TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
else
  TARGETS=("${ALL_TARGETS[@]}")
fi

step()  { printf '\n\033[1;36m▸ %s\033[0m\n' "$*"; }
ok()    { printf '\033[32m  ✓ %s\033[0m\n' "$*"; }
warn()  { printf '\033[33m  ⚠ %s\033[0m\n' "$*"; }
fail()  { printf '\033[31m  ✗ %s\033[0m\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# 1. Tag the source repo
# ---------------------------------------------------------------------------
cd "$REPO_ROOT"

# Guard: never release from a stale local main. Releases have been cut from a
# local main that was many commits behind origin/main — that either fails on a
# non-fast-forward push or quietly reverts upstream commits. Fetch and refuse
# if behind. Override with SKIP_SYNC_CHECK=1 for a deliberate offline build.
if [[ "${SKIP_SYNC_CHECK:-0}" != "1" ]]; then
  step "verify local branch is in sync with origin/main"
  if git fetch --quiet origin main 2>/dev/null; then
    behind="$(git rev-list --count HEAD..origin/main 2>/dev/null || echo 0)"
    if [[ "$behind" != "0" ]]; then
      fail "local branch is $behind commit(s) behind origin/main — run 'git pull --ff-only origin main' first (or SKIP_SYNC_CHECK=1 to override)"
    fi
    ok "in sync with origin/main"
  else
    warn "could not fetch origin (offline?) — skipping sync check"
  fi
fi

if [[ "${SKIP_TAG:-0}" != "1" ]]; then
  step "bump workspace version + tag $VERSION"
  if [[ -n "$(git status --porcelain Cargo.toml Cargo.lock 2>/dev/null)" ]]; then
    fail "Cargo.toml / Cargo.lock dirty — commit or stash first"
  fi
  # Idempotent: skip if Cargo.toml already at this version
  CUR=$(grep -E '^version = ' Cargo.toml | head -1 | sed -E 's/version = "(.+)"/\1/')
  if [[ "$CUR" != "$VER_NUM" ]]; then
    sed -i.bak -E "s/^version = \"$CUR\"/version = \"$VER_NUM\"/" Cargo.toml
    rm Cargo.toml.bak
    # Keep the sibling component versions in lockstep with the product version
    # (pure-Python client + private frontends — enforced by version-guard in CI).
    perl -i -pe 'if(!$d && s/^version = "[^"]*"/version = "'"$VER_NUM"'"/){$d=1}' bindings/python/pyproject.toml
    perl -i -pe 'if(!$d && s/("version":\s*)"[^"]*"/${1}"'"$VER_NUM"'"/){$d=1}' web/package.json
    perl -i -pe 'if(!$d && s/("version":\s*)"[^"]*"/${1}"'"$VER_NUM"'"/){$d=1}' website/package.json
    cargo check --workspace --release >/dev/null 2>&1 || \
      cargo check --workspace --release   # surface the real error if it fails
    git add Cargo.toml Cargo.lock bindings/python/pyproject.toml web/package.json website/package.json
    git commit -m "release: $VERSION" >/dev/null
    ok "bumped Cargo.toml + pyproject + web/website $CUR → $VER_NUM and committed"
  else
    ok "Cargo.toml already at $VER_NUM"
  fi
  if ! git rev-parse "$VERSION" >/dev/null 2>&1; then
    git tag -a "$VERSION" -m "$VERSION"
    ok "tagged $VERSION"
  else
    warn "tag $VERSION already exists; reusing"
  fi
  git push origin main "$VERSION" 2>&1 | tail -3 | sed 's/^/  /'
else
  ok "SKIP_TAG=1 — using HEAD; no version bump or tag"
fi

# ---------------------------------------------------------------------------
# 2. Build all requested targets
# ---------------------------------------------------------------------------
STAGE="$(mktemp -d -t ctxone-release-$VERSION.XXXXXX)"
trap 'rm -rf "$STAGE"' EXIT
# bash 3.2-compatible (no associative arrays): parallel array indexed
# alongside TARGETS. SHAS[i] is the sha256 of TARGETS[i]'s tarball.
SHAS=()

build_target() {
  local target="$1"
  step "build $target"
  case "$target" in
    *apple-darwin)
      cargo build --release --target "$target" --bin ctxone-hub --bin ctx
      ;;
    *linux-gnu)
      command -v cross >/dev/null || fail "cross not installed (cargo install cross)"
      docker info >/dev/null 2>&1 || fail "Docker not running (open -a Docker)"
      cross build --release --target "$target" --bin ctxone-hub --bin ctx
      ;;
    *)
      fail "unknown target: $target"
      ;;
  esac
}

tarball_target() {
  local target="$1"
  local stem="ctxone-$VERSION-$target"
  local src="$REPO_ROOT/target/$target/release"
  local d="$STAGE/$stem"
  mkdir -p "$d"
  cp "$src/ctxone-hub" "$src/ctx" "$d/"
  ( cd "$STAGE" && tar -czf "$stem.tar.gz" "$stem" )
  shasum -a 256 "$STAGE/$stem.tar.gz" | awk '{print $1}'
}

for i in "${!TARGETS[@]}"; do
  t="${TARGETS[$i]}"
  build_target "$t"
  SHAS[$i]=$(tarball_target "$t")
  ok "ctxone-$VERSION-$t.tar.gz  sha=${SHAS[$i]}"
done

# ---------------------------------------------------------------------------
# 3. GitHub release: create-or-reuse, then upload all assets
# ---------------------------------------------------------------------------
step "GitHub release $VERSION on $RELEASE_REPO"

# Switch to agentstatelabs for upload; restore prior account on exit
PRIOR_ACCOUNT=$(gh auth status 2>&1 | awk '/Active account: true/{f=1} /Logged in/{a=$NF} END{print a}' || echo "")
gh auth switch -u agentstatelabs >/dev/null

if gh release view "$VERSION" -R "$RELEASE_REPO" >/dev/null 2>&1; then
  ok "release $VERSION already exists; appending assets"
else
  gh release create "$VERSION" -R "$RELEASE_REPO" \
    --title "CTXone $VERSION" \
    --notes "Release built locally via scripts/release.sh. See CHANGELOG.md for details." \
    >/dev/null
  ok "created release"
fi

for t in "${TARGETS[@]}"; do
  asset="$STAGE/ctxone-$VERSION-$t.tar.gz"
  gh release upload "$VERSION" -R "$RELEASE_REPO" --clobber "$asset" >/dev/null
  ok "uploaded $(basename "$asset")"
done

if [[ -n "$PRIOR_ACCOUNT" && "$PRIOR_ACCOUNT" != "agentstatelabs" ]]; then
  gh auth switch -u "$PRIOR_ACCOUNT" >/dev/null 2>&1 || true
fi

# ---------------------------------------------------------------------------
# 4. Patch + push the brew formula
# ---------------------------------------------------------------------------
if [[ "${SKIP_FORMULA:-0}" == "1" ]]; then
  ok "SKIP_FORMULA=1 — leaving tap alone"
  exit 0
fi

step "update tap formula"
[[ -d "$TAP_ROOT" ]] || fail "tap clone not at $TAP_ROOT — clone it: \`git clone $TAP_GITLAB $TAP_ROOT\`"

cd "$TAP_ROOT"
git pull --ff-only origin main >/dev/null 2>&1 || true

FORMULA="$TAP_ROOT/Formula/ctxone.rb"
[[ -f "$FORMULA" ]] || fail "formula not at $FORMULA"

python3 - <<PY
import re
p = "$FORMULA"
src = open(p).read()
ver = "$VER_NUM"
src = re.sub(r'^(\s*version\s+)"[^"]*"', rf'\1"{ver}"', src, count=1, flags=re.M)
shas = [
$(for i in "${!TARGETS[@]}"; do printf '    ("%s", "%s"),\n' "${TARGETS[$i]}" "${SHAS[$i]}"; done)
]
for target, sha in shas:
    pat = re.compile(
        r'(url "https://github\.com/agentstatelabs/ctxone-releases/releases/download/)[^/]+/(ctxone-)[^"]+(-' +
        re.escape(target) + r'\.tar\.gz"\s+sha256 ")[^"]*(")'
    )
    repl = lambda m: m.group(1) + 'v' + ver + '/' + m.group(2) + 'v' + ver + m.group(3) + sha + m.group(4)
    src, n = pat.subn(repl, src, count=1)
    if n == 0:
        raise SystemExit(f"could not find URL+sha pair for {target} in formula")
open(p, "w").write(src)
PY

if [[ -n "$(git status --porcelain Formula/ctxone.rb)" ]]; then
  git add Formula/ctxone.rb
  git -c user.email="agentstatelabs@users.noreply.github.com" \
      -c user.name="agentstatelabs" \
      commit -m "ctxone $VER_NUM: bump version + sha256s" >/dev/null
  git push origin main 2>&1 | tail -2 | sed 's/^/  /'
  ok "formula updated and pushed (mirror replicates within ~seconds)"
else
  ok "formula already at $VERSION with matching sha256s"
fi

step "done — $VERSION shipped"
echo "  release:   https://github.com/$RELEASE_REPO/releases/tag/$VERSION"
echo "  install:   brew tap agentstatelabs/ctxone && brew install ctxone"

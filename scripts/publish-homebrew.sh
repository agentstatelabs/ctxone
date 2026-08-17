#!/usr/bin/env bash
#
# publish-homebrew — render the Homebrew formula for $CI_COMMIT_TAG and commit
# it to this product's tap on GitLab.
#
# Runs in GitLab CI, on tag pipelines only. Every hop is GitLab -> GitHub:
# this job reads the public release assets over HTTPS and writes the formula
# to a GitLab project. Nothing reaches back into GitLab from GitHub, and no
# internal hostname appears in a mirrored file — the tap URL is built from
# $CI_SERVER_URL, matching the pattern in agentstategraph/.gitlab-ci.yml.
#
# Pushing the tap's main fires ITS publish-github job, which leak-scans and
# force-pushes to the GitHub tap that `brew tap` actually reads. GitLab stays
# the source of truth; never write the GitHub tap directly, or the next tap
# change reverts it and leaves GitHub non-ff in between.
#
# Required env (set per-repo in .gitlab-ci.yml):
#   RELEASES_REPO   — GitHub "org/repo" holding the release assets
#   TAP_PATH        — GitLab "group/project" of the tap
#   FORMULA_PATH    — path within the tap, e.g. Formula/asd.rb
#   ARTIFACT_PREFIX — tarball stem, e.g. asd -> asd-<TAG>-<target>.tar.gz
#
# Required CI variables (group-level, protected):
#   GITLAB_RELEASE_TOKEN — write_repository on $TAP_PATH
#
# Exit: 0 pushed or already current, 1 assets never appeared, 2 misconfig.
set -euo pipefail

: "${CI_COMMIT_TAG:?this job only runs on tag pipelines}"
: "${CI_SERVER_URL:?missing CI_SERVER_URL (GitLab built-in)}"
: "${RELEASES_REPO:?set RELEASES_REPO}"
: "${TAP_PATH:?set TAP_PATH}"
: "${FORMULA_PATH:?set FORMULA_PATH}"
: "${ARTIFACT_PREFIX:?set ARTIFACT_PREFIX}"
: "${GITLAB_RELEASE_TOKEN:?set GITLAB_RELEASE_TOKEN (protected group variable — is this ref protected?)}"

TAG="$CI_COMMIT_TAG"
VER="${TAG#v}"

# Homebrew has no Windows support, so the msvc tarball is published for direct
# download only and is deliberately not a formula input.
TARGETS=(
  aarch64-apple-darwin
  x86_64-apple-darwin
  x86_64-unknown-linux-gnu
  aarch64-unknown-linux-gnu
)

BASE="https://github.com/${RELEASES_REPO}/releases/download/${TAG}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# --- wait for the GitHub build to publish the assets -------------------------
# The job is `when: delayed` so most of the build has already happened; this
# poll only covers the variance. Bounded so a failed matrix leg surfaces as a
# job failure instead of hanging the pipeline.
echo ">> waiting for ${TAG} assets on ${RELEASES_REPO}"
ready=0
for attempt in $(seq 1 40); do
  if curl -fsIL -o /dev/null "${BASE}/${ARTIFACT_PREFIX}-${TAG}-${TARGETS[0]}.tar.gz" 2>/dev/null; then
    ready=1
    echo "   assets present (attempt ${attempt})"
    break
  fi
  sleep 30
done
if [ "$ready" != "1" ]; then
  echo "   ${TAG} assets never appeared after 20 minutes." >&2
  echo "   Check the GitHub release workflow — a failed matrix leg leaves the" >&2
  echo "   release partially populated or absent." >&2
  exit 1
fi

# --- download + hash ---------------------------------------------------------
declare -a SHAS=()
for i in "${!TARGETS[@]}"; do
  t="${TARGETS[$i]}"
  f="${ARTIFACT_PREFIX}-${TAG}-${t}.tar.gz"
  curl -fsSL -o "${WORK}/${f}" "${BASE}/${f}" \
    || { echo "   missing asset: ${f}" >&2; exit 1; }
  SHAS[$i]="$(sha256sum "${WORK}/${f}" | awk '{print $1}')"
  echo "   ${t}  ${SHAS[$i]}"
done

# --- clone the tap from GitLab ----------------------------------------------
# CI_SERVER_URL keeps the hostname out of the repo; leak-scan BLOCKs literals.
TAP_URL="https://oauth2:${GITLAB_RELEASE_TOKEN}@${CI_SERVER_URL#https://}/${TAP_PATH}.git"
git clone --depth 1 "$TAP_URL" "${WORK}/tap" 2>&1 | sed 's/^/   /'

# --- render ------------------------------------------------------------------
FORMULA="${WORK}/tap/${FORMULA_PATH}"
[ -f "$FORMULA" ] || { echo "formula not found at ${FORMULA_PATH} in ${TAP_PATH}" >&2; exit 2; }

TARGETS_CSV="$(IFS=,; echo "${TARGETS[*]}")" \
SHAS_CSV="$(IFS=,; echo "${SHAS[*]}")" \
FORMULA="$FORMULA" TAG="$TAG" VER="$VER" \
RELEASES_REPO="$RELEASES_REPO" ARTIFACT_PREFIX="$ARTIFACT_PREFIX" \
python3 - <<'PY'
import os, re, sys

formula = os.environ["FORMULA"]
tag, ver = os.environ["TAG"], os.environ["VER"]
repo, prefix = os.environ["RELEASES_REPO"], os.environ["ARTIFACT_PREFIX"]
targets = os.environ["TARGETS_CSV"].split(",")
shas = os.environ["SHAS_CSV"].split(",")

src = open(formula).read()
src = re.sub(r'^(\s*version\s+)"[^"]*"', rf'\1"{ver}"', src, count=1, flags=re.M)

for t, sha in zip(targets, shas):
    pat = re.compile(
        r'(url "https://github\.com/' + re.escape(repo) +
        r'/releases/download/)[^/]+/(' + re.escape(prefix) + r'-)[^"]+(-' +
        re.escape(t) + r'\.tar\.gz"\s+sha256 ")[^"]*(")')
    src, n = pat.subn(
        lambda m: m.group(1) + tag + "/" + m.group(2) + tag + m.group(3) + sha + m.group(4),
        src, count=1)
    if n != 1:
        sys.exit(f"could not find url+sha256 pair for {t} in {formula}")

open(formula, "w").write(src)
PY

# --- commit + push -----------------------------------------------------------
cd "${WORK}/tap"
if [ -z "$(git status --porcelain "$FORMULA_PATH")" ]; then
  echo ">> formula already current for ${TAG} — nothing to push"
  exit 0
fi
git -c user.email="ci@agentstatelabs.com" -c user.name="agentstatelabs-ci" \
    commit -m "${ARTIFACT_PREFIX} ${VER}: bump version + sha256s" "$FORMULA_PATH"
git push origin HEAD:main 2>&1 | sed 's/^/   /'
echo ">> pushed — the tap's publish-github mirrors it to the GitHub tap"

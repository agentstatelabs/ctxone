#!/usr/bin/env bash
#
# stage-lens.sh — copy the built Lens frontend into server/lens/ so the
# published ctxone-hub crate can embed it.
#
# The Hub embeds web/build/ (src/lens.rs). That path is outside the crate, and
# cargo packages only files inside it, so before `cargo publish` the build is
# copied to server/lens/ (gitignored). server/build.rs embeds it from there when
# web/build/ is absent, as it is in the unpacked crate.
#
# Usage: scripts/stage-lens.sh            # npm ci + npm run build, then copy
#        SKIP_BUILD=1 scripts/stage-lens.sh   # copy an existing web/build/
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [ -z "${SKIP_BUILD:-}" ]; then
  (cd web && npm ci && npm run build)
fi
[ -f web/build/index.html ] || { echo "error: web/build/index.html missing; build the frontend first" >&2; exit 1; }

rm -rf server/lens
cp -R web/build server/lens
echo "staged $(find server/lens -type f | wc -l | tr -d ' ') files ($(du -sh server/lens | cut -f1)) into server/lens/"

#!/usr/bin/env bash
#
# Point git at the repo's tracked hooks (.githooks/) so the pre-push fmt gate
# runs for this clone. Run once after cloning:
#
#   scripts/setup-hooks.sh
#
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
git config core.hooksPath .githooks
chmod +x .githooks/* 2>/dev/null || true
echo "✓ core.hooksPath = .githooks (pre-push fmt gate active for this clone)"

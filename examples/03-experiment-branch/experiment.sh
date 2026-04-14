#!/bin/bash
# Try a risky memory change on a branch, diff it, then decide.
# Usage: ./experiment.sh <markdown-file>

set -e

if [ $# -lt 1 ]; then
    echo "Usage: $0 <markdown-file>" >&2
    exit 64
fi

DOC="$1"

if [ ! -f "$DOC" ]; then
    echo "File not found: $DOC" >&2
    exit 66
fi

if ! ctx status >/dev/null 2>&1; then
    echo "Hub unreachable. Run ctx serve --http first." >&2
    exit 69
fi

BRANCH="exp-$(date +%Y%m%d-%H%M)"

echo "=== Creating branch '$BRANCH' ==="
ctx branch "$BRANCH" --from main
echo

echo "=== Priming on the experiment branch ==="
ctx --branch "$BRANCH" prime "$DOC" --pin --source experiment
echo

echo "=== Diff main vs $BRANCH ==="
ctx diff main "$BRANCH"
echo

# Pick a query that should hit the primed content
QUERY=$(head -c 64 "$DOC" | tr -cd 'A-Za-z ' | awk '{print tolower($2)}')
QUERY=${QUERY:-context}

echo "=== Recall on main (query: '$QUERY') ==="
ctx --branch main recall "$QUERY"
echo

echo "=== Recall on $BRANCH (query: '$QUERY') ==="
ctx --branch "$BRANCH" recall "$QUERY"
echo

echo "=== Decision time ==="
echo "The experiment branch is at: $BRANCH"
echo
echo "To KEEP: continue working on it  → export CTX_BRANCH=$BRANCH"
echo "To DISCARD: just switch back     → export CTX_BRANCH=main"
echo "To INSPECT: ctx --branch $BRANCH ls /memory/pinned"

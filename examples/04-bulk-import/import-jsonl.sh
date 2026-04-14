#!/bin/bash
# Import facts from a JSONL file (one JSON object per line).
#
# Expected object shape:
#   {"fact": "...", "importance": "high|medium|low", "context": "...", "tags": ["..."]}
#
# Only `fact` is required. Others default.

set -e

if [ $# -lt 1 ]; then
    echo "Usage: $0 <file>" >&2
    exit 64
fi

FILE="$1"

if [ ! -f "$FILE" ]; then
    echo "File not found: $FILE" >&2
    exit 66
fi

if ! command -v jq >/dev/null 2>&1; then
    echo "jq is required for import-jsonl.sh. Install it first." >&2
    exit 70
fi

COUNT=$(grep -cv '^[[:space:]]*$' "$FILE" || true)
echo "Importing $COUNT facts from $FILE..."

IMPORTED=0
while IFS= read -r line; do
    [ -z "$line" ] && continue

    FACT=$(echo "$line" | jq -r '.fact // empty')
    if [ -z "$FACT" ]; then
        printf '?'
        continue
    fi

    IMP=$(echo "$line" | jq -r '.importance // "medium"')
    CTX=$(echo "$line" | jq -r '.context // "imported"')

    # jq @sh quotes each tag safely; fall back to no tags if missing
    TAGS_FLAGS=""
    if echo "$line" | jq -e '.tags | length > 0' >/dev/null 2>&1; then
        for tag in $(echo "$line" | jq -r '.tags[]'); do
            TAGS_FLAGS="$TAGS_FLAGS --tags $tag"
        done
    fi

    if ctx remember "$FACT" --importance "$IMP" --context "$CTX" $TAGS_FLAGS --format id > /dev/null 2>&1; then
        printf '.'
        IMPORTED=$((IMPORTED + 1))
    else
        printf 'x'
    fi
done < "$FILE"

echo " done"
echo "Imported $IMPORTED facts with per-fact metadata"

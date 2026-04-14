#!/bin/bash
# Import facts from a plain-text file, one per line.
# Usage: ./import-txt.sh <file>
#
# All facts get:
#   --context imported
#   --importance medium
#
# Edit these defaults in the ctx remember call below if you need something
# else globally.

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

COUNT=$(grep -cv '^[[:space:]]*$' "$FILE" || true)
echo "Importing $COUNT facts from $FILE..."

IMPORTED=0
while IFS= read -r line; do
    [ -z "$line" ] && continue
    [[ "$line" =~ ^[[:space:]]*# ]] && continue  # skip comments

    if ctx remember "$line" --importance medium --context imported --format id > /dev/null 2>&1; then
        printf '.'
        IMPORTED=$((IMPORTED + 1))
    else
        printf 'x'
    fi
done < "$FILE"

echo " done"
echo "Imported $IMPORTED facts under /memory/imported/"

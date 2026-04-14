#!/bin/bash
# Daily digest of CtxOne memory activity for the last 24 hours.
# Install: chmod +x this file, then wire to cron or call directly.

set -e

# Compute "24 hours ago" as an ISO 8601 UTC timestamp.
# macOS and Linux have different date command flags; try both.
if date -u -v-1d +"%Y-%m-%dT%H:%M:%SZ" >/dev/null 2>&1; then
    SINCE=$(date -u -v-1d +"%Y-%m-%dT%H:%M:%SZ")  # macOS / BSD
else
    SINCE=$(date -u -d '1 day ago' +"%Y-%m-%dT%H:%M:%SZ")  # GNU
fi

# Bail gracefully if the Hub is down.
if ! ctx status >/dev/null 2>&1; then
    echo "CtxOne Hub unreachable. No digest available." >&2
    exit 0
fi

# Fetch commits and filter to the last 24 hours, grouped by category.
DIGEST=$(ctx log -n 200 --format json 2>/dev/null | jq -r --arg since "$SINCE" '
    [.[] | select(.timestamp > $since)]
    | if length == 0 then
        "EMPTY"
      else
        group_by(.intent.category)
        | map(
            "[\(.[0].intent.category)]\n"
            + ([.[] | "  • \(.intent.description) (\(.agent_id))"] | join("\n"))
          )
        | join("\n\n")
      end
')

if [ "$DIGEST" = "EMPTY" ] || [ -z "$DIGEST" ]; then
    echo "No CtxOne activity in the last 24 hours."
    exit 0
fi

echo "CtxOne activity — last 24 hours"
echo
echo "$DIGEST"

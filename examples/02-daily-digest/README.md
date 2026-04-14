# Example 02 — Daily digest

A cron job that queries CtxOne's commit log for the last 24 hours and
emits a formatted digest — useful for standups, team updates, or "what
did I do yesterday?" self-checks.

## What it does

1. Reads the last 200 commits via `ctx log --format json`.
2. Filters to commits newer than 24 hours ago via `jq`.
3. Formats as a bullet list grouped by intent category.
4. Prints to stdout (or emails / Slack-posts — see the variations).

## Setup

Make the script executable and test it:

```bash
chmod +x daily-digest.sh
./daily-digest.sh
```

## Sample output

```
CtxOne activity — last 24 hours

[Checkpoint]
  • Shipped Postgres backend (ctxone-agent)
  • Decided to use BSL-1.1 for engine (ctxone-agent)

[Custom("Observe")]
  • CtxOne Hub wraps AgentStateGraph (ctxone)
  • Lens is built with SvelteKit (ctxone)
  • ctx init auto-configures Claude, Cursor (ctxone)
```

If nothing happened yesterday:

```
No CtxOne activity in the last 24 hours.
```

## Wire to cron

```cron
# 9:07 AM every day, email yourself the digest
7 9 * * *  /path/to/daily-digest.sh | mail -s "CtxOne digest" you@example.com
```

Or pipe to a Slack webhook:

```bash
/path/to/daily-digest.sh | curl -X POST -H "Content-type: application/json" \
  -d "{\"text\":\"$(cat)\"}" https://hooks.slack.com/services/your/webhook
```

## Why this is a good CtxOne pattern

- Zero parsing — `--format json` gives structured data
- Zero server changes — just a polling client
- Composable — pipes into any reporting tool
- Survivable — if the Hub is down, the script exits gracefully

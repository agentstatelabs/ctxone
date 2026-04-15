# Cookbook

Real-world recipes for putting CTXone into a developer workflow. Every recipe
is self-contained and copy-pasteable.

## Table of contents

1. [Prime your project README on every git push](#prime-your-project-readme-on-every-git-push)
2. [Daily digest of what changed in the last 24 hours](#daily-digest-of-what-changed-in-the-last-24-hours)
3. [Shell prompt that shows session savings](#shell-prompt-that-shows-session-savings)
4. [Capture a scripted fact in a shell variable](#capture-a-scripted-fact-in-a-shell-variable)
5. [Experiment on a branch, diff, then merge](#experiment-on-a-branch-diff-then-merge)
6. [Team-shared memory via Postgres](#team-shared-memory)
7. [Bulk-import facts from a file](#bulk-import-facts-from-a-file)
8. [Live-watch commits in a second terminal](#live-watch-commits-in-a-second-terminal)
9. [CI check: fail the build if a decision doesn't have a recorded reason](#ci-check-decision-reasoning)

---

## Prime your project README on every git push

Keep pinned memory in sync with your README so agents always see the current
canonical project context.

```bash
# .git/hooks/pre-push
#!/bin/bash
set -e

if ! command -v ctx >/dev/null; then
  exit 0  # silently skip if ctx isn't installed
fi

if ! ctx doctor >/dev/null 2>&1; then
  exit 0  # silently skip if hub isn't running
fi

ctx prime ./README.md --pin --source project > /dev/null
echo "ctxone: primed README as pinned project context"
```

Make it executable:

```bash
chmod +x .git/hooks/pre-push
```

Now every time you `git push`, the README's H1/H2 sections are re-stored as
pinned memory under `/memory/pinned/project/*`. Because prime is idempotent
by source, re-running doesn't create duplicates.

**Variation:** to prime multiple docs:

```bash
ctx prime ./README.md --pin --source readme
ctx prime ./docs/ARCHITECTURE.md --pin --source architecture
ctx prime ./docs/DECISIONS.md --source decisions  # searchable, not pinned
```

---

## Daily digest of what changed in the last 24 hours

A cron job that asks "what happened yesterday?" and emails or posts it.

```bash
#!/bin/bash
# /usr/local/bin/ctxone-daily-digest.sh
set -e

SINCE=$(date -u -v-1d +"%Y-%m-%dT%H:%M:%SZ")  # macOS
# SINCE=$(date -u -d '1 day ago' +"%Y-%m-%dT%H:%M:%SZ")  # Linux

DIGEST=$(ctx log -n 200 --format json | \
  jq -r --arg since "$SINCE" '
    .[]
    | select(.timestamp > $since)
    | "- [\(.intent.category)] \(.intent.description) (\(.agent_id))"
  ')

if [ -z "$DIGEST" ]; then
  echo "No activity in the last 24 hours."
  exit 0
fi

echo "CTXone activity since $SINCE:"
echo
echo "$DIGEST"
```

Wire it to cron:

```cron
# crontab -e
7 9 * * * /usr/local/bin/ctxone-daily-digest.sh | mail -s "CTXone digest" you@example.com
```

**Why `--format json | jq`:** the Hub returns structured commit data with
intent category, description, agent ID, and timestamp. jq filters to
yesterday and pretty-prints. No string scraping.

---

## Shell prompt that shows session savings

Put cumulative token savings in your shell prompt so you see it every time
you hit Enter.

```bash
# ~/.zshrc
_ctxone_prompt_info() {
  local saved=$(ctx stats --format json 2>/dev/null | jq -r '.session_tokens_saved // "—"')
  if [ "$saved" != "—" ] && [ "$saved" != "null" ]; then
    echo "ctx:${saved}"
  fi
}

PROMPT='%F{blue}$(_ctxone_prompt_info)%f %~ $ '
```

Result:

```
ctx:1706 ~/projects/myapp $
```

The `2>/dev/null` silently hides output when the Hub is down — your prompt
keeps working regardless.

**Performance note:** `ctx stats` makes one HTTP call per prompt render. If
that's too much, cache for 10 seconds:

```bash
_ctxone_cached_saved() {
  local cache=/tmp/ctxone_saved.$USER
  if [ -f "$cache" ] && [ $(($(date +%s) - $(stat -f %m "$cache" 2>/dev/null || stat -c %Y "$cache"))) -lt 10 ]; then
    cat "$cache"
  else
    ctx stats --format json 2>/dev/null | jq -r '.session_tokens_saved // "—"' > "$cache"
    cat "$cache"
  fi
}
```

---

## Capture a scripted fact in a shell variable

Use `--format id` to grab the path or commit id of a newly-stored fact.

```bash
# Remember a fact and capture its path
path=$(ctx remember "Deployment finished for build #4721" \
       --importance high \
       --context ops \
       --format id)

echo "stored at $path"
# stored at sg_a1b2c3d4e5f6   (commit id)
```

To capture the path instead of the commit id, parse the JSON directly:

```bash
result=$(ctx remember "..." --format json)
path=$(echo "$result" | jq -r .path)
commit=$(echo "$result" | jq -r .commit_id)
```

Then you can forget it later:

```bash
ctx forget "$path" --reason "deploy rolled back"
```

This is the loop that enables transactional scripts: remember, check, optionally
forget on failure.

---

## Experiment on a branch, diff, then merge

Try a risky memory change (bulk prime, reorganization) on a branch without
touching main.

```bash
# Snapshot current state as a branch
ctx branch experiment

# Work on the branch
ctx --branch experiment prime ./big-proposal.md --pin --source proposal
ctx --branch experiment remember "New naming convention: verb-noun" --context conventions

# See what changed vs main
ctx diff main experiment

# Try recalling on the experiment branch
ctx --branch experiment recall "naming"

# If you like it: merge (there's no merge command yet — use Repository::merge
# via the engine directly, or just keep writing to the experiment branch)

# If you don't: switch back
export CTX_BRANCH=main
```

**Tip:** use `CTX_BRANCH` env var to avoid typing `--branch experiment` on
every command during a session.

---

## Team-shared memory

Run the Hub against a Postgres instance so multiple developers (or multiple
machines) see the same memory graph.

```bash
# Start the Hub pointing at Postgres
export DATABASE_URL=postgres://ctxone:secret@db.internal:5432/ctxone
ctx serve --http --storage postgres
```

Or via Docker:

```yaml
# docker-compose.yml
services:
  db:
    image: postgres:16
    environment:
      POSTGRES_USER: ctxone
      POSTGRES_PASSWORD: secret
      POSTGRES_DB: ctxone
    volumes:
      - ctxone-pg-data:/var/lib/postgresql/data
    ports:
      - "5432:5432"

  hub:
    image: ghcr.io/ctxone/ctxone:latest
    command: ["ctxone-hub", "--http", "--port", "3001", "--storage", "postgres"]
    environment:
      DATABASE_URL: postgres://ctxone:secret@db:5432/ctxone
    depends_on: [db]
    ports:
      - "3001:3001"

volumes:
  ctxone-pg-data:
```

Each team member points their CLI at the shared Hub:

```bash
export CTX_SERVER=http://hub.internal:3001
ctx remember "..."  # writes to the shared graph
```

**Separation:** use branches to give each agent or project its own namespace:

```bash
ctx --branch team/backend remember "..."
ctx --branch team/frontend remember "..."
```

**Caveat:** Postgres auth in the Hub is single-tenant right now. For
multi-tenant deployments with per-team isolation, wait for a future release
or run separate Hub instances.

---

## Bulk-import facts from a file

Prime imports markdown structured by headings. For unstructured plain-text
facts (one per line), loop:

```bash
# facts.txt: one fact per line
while IFS= read -r fact; do
  [ -z "$fact" ] && continue
  ctx remember "$fact" --importance medium --context imported --format id
done < facts.txt
```

Or from a JSONL file with explicit metadata:

```bash
# facts.jsonl: one JSON object per line
# {"fact":"...","importance":"high","context":"..."}
while IFS= read -r line; do
  fact=$(echo "$line" | jq -r .fact)
  imp=$(echo "$line" | jq -r '.importance // "medium"')
  ctx=$(echo "$line" | jq -r '.context // "imported"')
  ctx remember "$fact" --importance "$imp" --context "$ctx" > /dev/null
done < facts.jsonl

echo "Imported $(wc -l < facts.jsonl) facts"
```

Or pipe a single fact from another command:

```bash
# Pipe from curl
curl -s https://example.com/status | ctx remember - --context status

# Pipe from stdin interactively
echo "Craig makes the best spaghetti" | ctx remember - --importance low --context trivia
```

---

## Live-watch commits in a second terminal

Open two terminals. In the first:

```bash
ctx tail
```

In the second, do anything that writes:

```bash
ctx remember "test fact"
ctx prime ./README.md
ctx demo
```

Each commit appears in the tail terminal within the poll interval (default
2000ms, override with `--interval 500` for snappier feedback).

**Use case:** a pair-programming session where one person runs `ctx tail`
and the other uses Claude Code. You watch the memory graph update in
real time.

---

## CI check: decision reasoning

Enforce that every `Checkpoint` commit has reasoning attached. Runs in CI
and fails if a decision was made without explanation.

```bash
#!/bin/bash
# scripts/check-decision-reasoning.sh
set -e

BAD=$(ctx log -n 1000 --format json | jq -r '
  [.[] | select(.intent.category == "Checkpoint")
       | select((.reasoning // "") == "")
       | .id] | join("\n")
')

if [ -n "$BAD" ]; then
  echo "ERROR: Checkpoint commits without reasoning:"
  echo "$BAD"
  exit 1
fi

echo "OK: all checkpoints have reasoning"
```

Wire it into your CI pipeline. The Hub needs to be reachable from the CI
runner — run it as a sidecar container or use a shared hosted instance.

---

## More ideas

- **Session replay**: use `ctx log` + `ctx recall` to reconstruct a session's
  memory state at a past moment
- **Memory audit**: periodic `ctx search` for stale or wrong facts, then `ctx forget`
- **Cross-tool sync**: a Git pre-commit hook that uses `ctx search` to find
  related past decisions and prints them as a reviewer checklist
- **Agent authority chains**: write each agent to a namespaced branch
  (`agents/alice`, `agents/bob`) so `ctx blame` shows which agent decided
  what

If you build something, open a PR adding it here.

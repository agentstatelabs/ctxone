# Example 01 — Git pre-push hook

Prime your project README as pinned memory on every `git push`, so every
agent session you open afterwards sees the current canonical context.

## What it does

On every `git push`:
1. Check that `ctx` is installed and the Hub is reachable.
2. Run `ctx prime README.md --pin --source project`.
3. Print a confirmation.

Prime is idempotent by source name, so re-running doesn't duplicate — it
overwrites. You always have exactly one copy of your README's H1/H2
sections in pinned memory.

## Setup

Copy `pre-push` into your repo's git hooks directory and make it
executable:

```bash
cp pre-push /path/to/your/repo/.git/hooks/pre-push
chmod +x /path/to/your/repo/.git/hooks/pre-push
```

## Run it

```bash
cd /path/to/your/repo
git commit --allow-empty -m "trigger pre-push"
git push
```

You should see:

```
ctxone: primed README as pinned project context
```

Verify:

```bash
ctx pinned
```

```
[project]
  <your first H1 or H2>
    <first two body lines>
    ...
  ...
```

## Why this works

- **Pinned** — always included in every recall, regardless of topic
- **`--source project`** — gives it a stable group name
- **Idempotent** — re-priming replaces, doesn't append
- **Silent failures** — if ctx or the Hub is unavailable, the hook exits 0
  so git push still works

## Caveats

- Only splits at H1 and H2 headings. Deeper headings become part of the
  parent section's body.
- Runs synchronously. For large READMEs, this adds ~100ms to your push.

# 5-Minute Quickstart

From nothing to seeing live token savings in about five minutes.

## 1. Install

**macOS / Linux:**

```bash
curl -sSL https://raw.githubusercontent.com/ctxone/ctxone/main/install.sh | sh
```

This drops `ctx` and `ctxone-hub` in `~/.local/bin`. If `~/.local/bin` isn't on
your `PATH`, add it:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

**Windows (PowerShell):**

```powershell
iwr https://raw.githubusercontent.com/ctxone/ctxone/main/install.ps1 | iex
```

This drops `ctx.exe` and `ctxone-hub.exe` in `%LOCALAPPDATA%\ctxone\bin` and
adds that directory to your user PATH. Open a **new** PowerShell window after
install for the PATH change to take effect.

**Verify:**

```bash
ctx --version
# ctx 0.60.0
```

## 2. Start the Hub

In one terminal:

```bash
ctx serve --http
```

You'll see:

```
Starting CTXone Hub on port 3001 (db: /Users/user/.ctxone/memory.db)
CTXone Hub v0.5.0
Storage: /Users/user/.ctxone/memory.db
HTTP API listening on http://0.0.0.0:3001
```

Leave it running. Open a second terminal for the rest of this guide.

## 3. Check everything is healthy

```bash
ctx doctor
```

You should see green checkmarks for the hub binary, the db path, and the HTTP
endpoint. The MCP config checks will be red until step 6.

## 4. Seed realistic data and see the savings

```bash
ctx demo
```

This writes 21 realistic facts (licensing, architecture, features, economics,
team) and runs four recalls, showing per-query and cumulative savings:

```
  recall "licensing"    →  2 matches, 34 tokens sent vs 451 flat (13.0x savings)
  recall "architecture" →  1 matches, 13 tokens sent vs 451 flat (32.8x savings)
  recall "tokens"       →  1 matches, 26 tokens sent vs 451 flat (17.4x savings)
  recall "Lens"         →  1 matches, 25 tokens sent vs 451 flat (17.5x savings)

Cumulative savings this session:
  98 tokens sent, 1706 tokens saved, 18.4x overall
```

**That's the whole pitch in one command.** Each recall returned exactly the facts
relevant to its topic — not the whole 451-token flat memory.

## 5. Try it yourself

```bash
ctx remember "We use BSL-1.1 for all projects" --importance high --context licensing
ctx recall "licensing"
```

You'll see the new fact plus the two demo licensing facts, with an updated
savings ratio.

Want to see the whole graph?

```bash
ctx ls /memory           # list all paths
ctx search "BSL"         # literal substring search
ctx log -n 5             # recent commit history
```

Want to see commits as they happen? Open a third terminal and run:

```bash
ctx tail
```

Then in your second terminal, run a few more `ctx remember` commands. The
tail will show each new commit within a second or two.

## 6. Wire it into your AI tools

```bash
ctx init
```

This auto-detects Claude Code, Claude Desktop, Cursor, VS Code, and Codex on
your machine and writes the MCP config for each (with your confirmation):

```
Detected AI tools:
  ✓ Claude Code
  ✓ Cursor
  ✗ Codex

Install CTXone MCP server into these tools? [Y/n] y
  → Claude Code: wrote .mcp.json ✓
  → Cursor: wrote .cursor/mcp.json ✓

CTXone is ready. Try: "remember that we use BSL-1.1 licensing"
```

After this, Claude Code / Cursor / etc. will call CTXone's MCP tools
(`remember`, `recall`, `prime`, etc.) automatically. Every session starts
with pinned context loaded and topic-relevant memories at hand — no more
re-explaining your project.

## 7. Prime your project's critical context

If your project has a `README.md`, pin its sections so every AI session you
open sees them:

```bash
ctx prime ./README.md --pin --source my-project
ctx pinned    # verify what's stored
```

Now every `ctx recall`, regardless of topic, returns those pinned sections
first — the "critical context for all calls" pattern.

## Next steps

- [Architecture](ARCHITECTURE.md) — the mental model for how recall and priming work
- [Token Savings](TOKEN_SAVINGS.md) — how the ratio is computed and how to maximize it
- [Cookbook](COOKBOOK.md) — real-world recipes (git hooks, cron jobs, shell prompts)

## Troubleshooting

**`ctx doctor` shows the hub as unreachable?**
You probably haven't started it. Run `ctx serve --http` in another terminal.

**`ctx --version` says "command not found"?**
`~/.local/bin` isn't on your PATH. Either add it or use the full path.

**I want to nuke the memory and start over.**
Stop the hub, delete `~/.ctxone/memory.db`, start the hub again.

**I want to share memory across a team.**
Use the Postgres backend: `ctx serve --http --storage postgres --database-url postgres://...`
See the [Cookbook](COOKBOOK.md#team-shared-memory) for the full recipe.

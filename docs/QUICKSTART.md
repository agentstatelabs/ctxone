# 5-Minute Quickstart

From nothing to seeing live token savings in about five minutes.

## 1. Install

**macOS (Homebrew — recommended):**

```bash
brew install ctxone/tap/ctxone
```

**macOS / Linux** (one-liner):

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
# ctx 0.75.1
```

## 2. Start the Hub

In one terminal:

```bash
ctx serve --http           # REST API + MCP at /mcp
ctx serve --http --lens    # …plus the Lens web UI at http://localhost:3001
```

A single `--http` hub serves the REST API, the MCP tool surface (`/mcp`), and —
with `--lens` — the web UI, all from one process. To run it at boot so it owns
the db before any agent, see [`ctx service install`](DEPLOYMENT.md#running-as-a-service).

You'll see:

```
Starting CtxOne Hub on port 3001 (db: /Users/user/.ctxone/memory.db)
INFO CtxOne Hub starting version="0.75.1"
INFO Storage: sqlite path=/Users/user/.ctxone/memory.db
INFO HTTP API listening port=3001
INFO Try: curl http://localhost:3001/api/health
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

### stdio (default) vs. one shared daemon

By default each tool spawns its **own** `ctxone-hub` over stdio. That's the
simplest setup, but only one process can own the db at a time, so you can't also
run a web UI on the same memory. To have **one** hub serve MCP + REST + Lens for
every tool, run the daemon (`ctxone-hub --http --lens`, ideally via
[`ctx service install`](DEPLOYMENT.md#running-as-a-service)) and point tools at it
by URL:

```bash
ctx init --transport http          # write URL configs instead of stdio spawns
```

`ctx init` picks the right shape per tool automatically — native
`{"type":"http","url":…}` for Claude Code / Cursor / VS Code, a `url` entry for
Codex, and an `mcp-remote` bridge for Claude Desktop (which has no native HTTP
MCP). See [DEPLOYMENT.md](DEPLOYMENT.md) for when to choose which, and for
`--auth-token` if you expose the hub beyond localhost.

## 7. Prime your project's critical context

If your project has a `README.md`, pin its sections so every AI session you
open sees them:

```bash
ctx prime ./README.md --pin --source my-project
ctx pinned    # verify what's stored
```

Now every `ctx recall`, regardless of topic, returns those pinned sections
first — the "critical context for all calls" pattern.

## 8. Give each repo its own namespace

By default everything lands in one shared namespace. If you work across
multiple repos, register each one as a **project** so its branches,
plans, and memory stay isolated:

```bash
cd ~/code/myrepo
ctx project add myrepo
```

This creates a `myrepo` namespace on the Hub and writes a `.ctxproject`
marker at the repo root — **commit that file** so every checkout (and
every agent) auto-detects the project. From then on, `ctx` commands run
inside the repo target its namespace automatically; `ctx status` and
`ctx project detect` show which one is active.

Anything you stored before adopting projects stays in the reserved
`default` namespace — nothing migrates, and it's still reachable with
`ctx --namespace default ...`.

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
Use the Postgres backend: `export DATABASE_URL=postgres://... && ctx serve --http --storage postgres` (the connection string is read from the `DATABASE_URL` env var, not a flag).
See the [Cookbook](COOKBOOK.md#team-shared-memory) for the full recipe.

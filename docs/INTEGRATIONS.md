# Integrating CTXone with AI Tools

How to wire CTXone into the common AI coding tools so every session starts
with project context loaded. For details on what each MCP tool does, see
[MCP_TOOLS.md](MCP_TOOLS.md).

**Chat hosts (not coding tools):** for Open WebUI specifically, CTXone
ships native Tool and Filter plugins — see [OPENWEBUI.md](OPENWEBUI.md).
The Filter auto-injects relevant memory into every turn, which works
even with models that don't support tool-calling.

## The fastest path

**Start the hub, then run `ctx init`.** `ctx init` points every supported AI
tool at your running hub (default `--transport http`), so start it first:

```bash
ctxone-hub --http --lens          # or: ctx service install (runs it at boot)
ctx init                          # auto-detects tools, writes URL configs
```

`ctx init` auto-detects every supported AI tool on your machine and writes the
MCP config for each (after your confirmation):

```
Detected AI tools:
  ✓ Claude Code
  ✓ Cursor
  ✓ VS Code
  ✗ Codex

Install CTXone MCP server into these tools? [Y/n] y
  → Claude Code: wrote .mcp.json ✓
  → Cursor: wrote .cursor/mcp.json ✓
  → VS Code: wrote .vscode/mcp.json ✓
```

That's it. Restart the AI tool and CTXone is live. The rest of this doc
explains what `ctx init` actually writes and how to do it manually.

## What `ctx init` writes

### http (default) — point tools at one shared daemon

`ctx init` points each tool at a **single running hub** (`ctxone-hub --http
--lens`) by URL, so nothing spawns its own child and one process serves MCP +
REST + Lens for every tool. It picks the correct shape per tool:

| Tool | What gets written |
|------|-------------------|
| Claude Code, Cursor, VS Code | native `{"type":"http","url":"http://localhost:3001/mcp?namespace=…"}` |
| Codex | `[mcp_servers.ctxone]` with `url = "http://localhost:3001/mcp?namespace=…"` |
| Claude Desktop | `mcp-remote` stdio bridge (`npx -y mcp-remote <url> --transport http-only`) — it has no native HTTP MCP |

The project namespace (detected from the cwd) is baked into the URL so the daemon
scopes writes per project. Override the endpoint with `--mcp-url`. If the hub
isn't running, `ctx init` warns you. See [DEPLOYMENT.md](DEPLOYMENT.md) for the
daemon setup, `ctx service install`, and `--auth-token` (with `--auth-token` /
`--auth-token-env` on `ctx init` to embed it) when exposing the hub beyond
localhost.

### stdio (alternative) — each tool spawns its own hub

`ctx init --transport stdio` writes a spawn config instead — each tool runs its
own `ctxone-hub` child (zero-setup, but one owner per db, no shared web UI):

```json
{
  "mcpServers": {
    "ctxone": {
      "command": "/Users/user/.local/bin/ctxone-hub",
      "args": ["--path", "/Users/user/.ctxone/memory.db"]
    }
  }
}
```

The `--path` flag is a canonical shared location so every AI tool talks to
the **same memory graph**. Without it, each tool would spawn the Hub with a
different default database depending on its working directory — the shared
memory promise would break.

---

## Claude Code

### Install scope

Claude Code reads `.mcp.json` from the project directory. `ctx init` writes
to `$PWD/.mcp.json` by default. For user-wide install (any project Claude
Code opens), use:

```bash
ctx init --global --tool claude
```

This writes to `~/.claude/settings.json` (merged with existing settings).

### Verifying

Restart Claude Code, then ask it:

> What MCP tools do you have available?

It should list `remember`, `recall`, `prime`, `context`,
`summarize_session`, `what_changed_since`, `why_did_we`, and the plan/docs
tools `plan_link`, `plan_stale`, and `docs_find`.

### Typical session

With CTXone configured, a Claude Code session looks like:

1. You open the project.
2. Claude Code calls `recall "<your first question>"` — gets pinned project
   context plus any facts relevant to the topic.
3. You work on something. Claude Code calls `remember "..."` when it
   learns a decision or a fact worth persisting.
4. At session end, Claude Code calls `summarize_session` with the highlights.
5. Next time you open the project, step 2 returns those summaries. **No
   re-explaining.**

Claude Code is the tool CTXone was designed for. Expect the best experience
here.

---

## Cursor

### Install scope

Cursor reads MCP config from either:
- `.cursor/mcp.json` in the project directory, or
- `~/.cursor/mcp.json` globally

`ctx init` writes to the project scope by default. For global:

```bash
ctx init --global --tool cursor
```

### Verifying

Open Cursor's Settings → Features → MCP Servers. You should see `ctxone`
listed. If you don't, check `.cursor/mcp.json` exists and has the right
shape.

### Notes

Cursor's MCP integration is newer than Claude Code's. Some edge cases:

- MCP tools may need to be toggled on in Cursor settings even after writing
  the config file.
- Cursor sometimes caches MCP tool lists — restart the app if new tools
  don't appear.

---

## VS Code (Copilot with MCP)

### Install scope

VS Code's MCP support is via Copilot and reads from `.vscode/mcp.json` in
the workspace. `ctx init` writes there by default.

For user settings (across all workspaces):

```bash
ctx init --global --tool vscode
```

This writes to `~/Library/Application Support/Code/User/settings.json` on
macOS (or the Linux/Windows equivalent).

### Verifying

Open the command palette → "MCP: List Servers". `ctxone` should appear.

### Notes

- MCP support in VS Code is still evolving. If your Copilot version doesn't
  support MCP, update to the latest.
- The tool list is exposed through Copilot's chat; typing `@ctxone` may or
  may not work depending on version.

---

## Codex (OpenAI CLI)

### Install scope

Codex uses TOML configuration in `~/.codex/config.toml`. `ctx init` writes
the `[mcp_servers.ctxone]` entry automatically, merging with any existing
settings (project trust level, other MCP servers like linear or figma)
without clobbering them.

```bash
ctx init --tool codex
```

### What gets written

Default (`--transport http`) — Codex supports HTTP MCP natively via a `url` key,
so it connects to your running daemon:

```toml
[mcp_servers.ctxone]
url = "http://localhost:3001/mcp?namespace=<detected>"
```

With `--transport stdio`, Codex spawns its own hub child instead:

```toml
[mcp_servers.ctxone]
command = "/Users/user/.local/bin/ctxone-hub"
args = ["--path", "/Users/user/.ctxone/memory.db"]
```

Codex picks up the config on next launch.

### Verifying

Run `codex` and check the MCP server list (command varies by Codex
version).

---

## Gemini CLI (Google)

### Install scope

Gemini CLI stores settings in `~/.gemini/settings.json`. The MCP server
section uses the same `mcpServers` JSON format as Claude Code.

```bash
# Project-level
ctx init --tool gemini

# User-level (recommended for a tool you use across projects)
ctx init --global --tool gemini
```

`ctx init` merges into the existing file, preserving `theme`, `model`,
and any other settings plus any pre-existing `mcpServers` entries.

### Verifying

Start a Gemini session and check the MCP server list from the CLI's
built-in diagnostics. The ctxone entry should appear — a `url` pointing at
your running hub under the default http transport, or a `command` spawning
`ctxone-hub` under `--transport stdio`.

---

## Grok CLI (xAI)

### Install scope

Grok CLI (`superagent-ai/grok-cli`) uses `.grok/settings.json` with an
`mcpServers` object — the same JSON shape as Claude Code and Gemini.

```bash
# Project-level
ctx init --tool grok

# User-level
ctx init --global --tool grok
```

### Verifying

Inside the Grok TUI, type `/mcps` to list configured MCP servers. The
`ctxone` entry should appear — connecting to your running hub by URL under
the default http transport, or spawning the Hub on session start under
`--transport stdio`.

---

## Generic fallback — any other MCP client

If you're using an MCP client that `ctx init` doesn't know about yet,
use `--config-path` to write a standard `mcpServers` JSON config to any
location:

```bash
ctx init --config-path ~/.myeditor/mcp.json
ctx init --config-path .vscode/mcp.json   # also works for VS Code Copilot
```

`ctx init` writes the same JSON shape every supported tool uses:

```json
{
  "mcpServers": {
    "ctxone": {
      "command": "/Users/user/.local/bin/ctxone-hub",
      "args": ["--path", "/Users/user/.ctxone/memory.db"]
    }
  }
}
```

If the target file already exists, `ctx init` merges into it — your
other MCP servers are preserved.

Use this when:
- A new MCP client ships before `ctx init` learns about it
- You have a custom tool or in-house editor that speaks MCP
- You want to write to a non-standard path for your team's conventions

---

## Claude Desktop

Unlike Claude Code, Claude Desktop (the chat app) uses a different config
path:

```
~/Library/Application Support/Claude/claude_desktop_config.json
```

`ctx init` writes here when it detects Claude Desktop is installed. The
format is the same `mcpServers` object.

### Notes

- Claude Desktop loads config once at startup. Restart the app after
  writing the config.
- Desktop tool use is more limited than Claude Code's; some tools (like
  `prime`, which takes a structured array) may be awkward to invoke
  interactively from chat.

---

## Any other MCP client

Any tool that reads an MCP server config file with the standard
`mcpServers` object format will work. The minimum config:

```json
{
  "mcpServers": {
    "ctxone": {
      "command": "ctxone-hub",
      "args": ["--path", "/absolute/path/to/memory.db"]
    }
  }
}
```

Notes:

- **`command`** — absolute path or a name on `PATH`. Use absolute paths in
  production to avoid surprises.
- **`args`** — always include `--path` pointing at a shared location. This
  is what makes memory shared across tools.
- **Transport** — if your client speaks HTTP MCP, prefer the daemon: point
  it at `http://localhost:3001/mcp?namespace=<ns>` (run `ctxone-hub --http`).
  The stdio shape above is the per-client fallback — the Hub speaks stdio MCP
  when spawned this way (no `--http`), and `--http` serves MCP at `/mcp`
  alongside the REST API, not instead of it.
- **Single-session (stdio only)** — a stdio Hub handles one client at a time
  and exits when the tool exits, so each session gets a fresh Hub process. The
  `--http` daemon has no such limit — every tool shares the one process.

---

## Sharing memory across sessions

Because every tool talks to the same `~/.ctxone/memory.db`, facts you store
in Claude Code are immediately visible in Cursor, and vice versa. This is
the entire point — no more per-tool memory silos.

If you don't want a tool to share, point its config at a different path:

```json
{
  "mcpServers": {
    "ctxone": {
      "command": "/Users/user/.local/bin/ctxone-hub",
      "args": ["--path", "/Users/user/.ctxone/isolated.db"]
    }
  }
}
```

---

## Sharing memory across team members

Run the Hub against Postgres and point every team member's tools at a
shared host:

```json
{
  "mcpServers": {
    "ctxone": {
      "command": "ctxone-hub",
      "args": [
        "--storage", "postgres",
        "--database-url", "postgres://ctxone:secret@db.internal:5432/ctxone"
      ]
    }
  }
}
```

See [COOKBOOK.md — Team-shared memory](COOKBOOK.md#team-shared-memory) for a
full docker-compose setup.

---

## Troubleshooting

**The tool says CTXone isn't configured, even after `ctx init`.**
Restart the tool. Most MCP clients load config at startup.

**Hub spawns but no memory is shared.**
Check each tool's config file and verify `--path` points at the same
absolute path. `ctx init --dry-run` shows exactly what gets written.

**The tool lists CTXone but tool calls fail.**
Run `ctx doctor` — it catches most infrastructure issues. If doctor is
green, check the Hub logs (stderr of the spawned process) via your tool's
MCP diagnostic view.

**Codex isn't auto-configured.**
Known limitation; write the TOML manually (see above).

For more, see [TROUBLESHOOTING.md](TROUBLESHOOTING.md).

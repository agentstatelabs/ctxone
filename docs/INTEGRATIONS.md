# Integrating CtxOne with AI Tools

How to wire CtxOne into the common AI coding tools so every session starts
with project context loaded. For details on what each MCP tool does, see
[MCP_TOOLS.md](MCP_TOOLS.md).

## The fastest path

**Run `ctx init`.** It auto-detects every supported AI tool on your machine
and writes the MCP config for each (after your confirmation):

```bash
ctx init
```

```
Detected AI tools:
  ✓ Claude Code
  ✓ Cursor
  ✓ VS Code
  ✗ Codex

Install CtxOne MCP server into these tools? [Y/n] y
  → Claude Code: wrote .mcp.json ✓
  → Cursor: wrote .cursor/mcp.json ✓
  → VS Code: wrote .vscode/mcp.json ✓
```

That's it. Restart the AI tool and CtxOne is live. The rest of this doc
explains what `ctx init` actually writes and how to do it manually.

## What `ctx init` writes

Each tool gets a JSON file with an `mcpServers.ctxone` entry:

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
`summarize_session`, `what_changed_since`, and `why_did_we`.

### Typical session

With CtxOne configured, a Claude Code session looks like:

1. You open the project.
2. Claude Code calls `recall "<your first question>"` — gets pinned project
   context plus any facts relevant to the topic.
3. You work on something. Claude Code calls `remember "..."` when it
   learns a decision or a fact worth persisting.
4. At session end, Claude Code calls `summarize_session` with the highlights.
5. Next time you open the project, step 2 returns those summaries. **No
   re-explaining.**

Claude Code is the tool CtxOne was designed for. Expect the best experience
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

Codex uses TOML configuration in `~/.codex/config.toml`. `ctx init`
currently **does not** write Codex configs automatically — it prints a
manual instruction. This is a known gap tracked for a future release.

### Manual setup

Add to `~/.codex/config.toml`:

```toml
[mcp_servers.ctxone]
command = "/Users/user/.local/bin/ctxone-hub"
args = ["--path", "/Users/user/.ctxone/memory.db"]
```

Codex will pick up the config on next launch.

### Verifying

Run `codex` and check the MCP server list (command varies by Codex
version).

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
- **Stdio transport** — CtxOne Hub speaks stdio MCP by default. Don't pass
  `--http`; that's for the REST API.
- **Single-session** — the Hub handles one stdio client at a time. When
  the AI tool exits, the Hub exits with it. Each tool session gets a fresh
  Hub process.

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

**The tool says CtxOne isn't configured, even after `ctx init`.**
Restart the tool. Most MCP clients load config at startup.

**Hub spawns but no memory is shared.**
Check each tool's config file and verify `--path` points at the same
absolute path. `ctx init --dry-run` shows exactly what gets written.

**The tool lists CtxOne but tool calls fail.**
Run `ctx doctor` — it catches most infrastructure issues. If doctor is
green, check the Hub logs (stderr of the spawned process) via your tool's
MCP diagnostic view.

**Codex isn't auto-configured.**
Known limitation; write the TOML manually (see above).

For more, see [TROUBLESHOOTING.md](TROUBLESHOOTING.md).

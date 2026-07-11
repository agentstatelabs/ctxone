# Deployment & Configuration

How to run the CTXone Hub, and the configuration choices that matter. If you just
installed and everything works, you don't need this page — it's for when you want
the **web UI**, want to run **multiple agents**, or want each agent to have its
**own memory**.

## The one rule that governs every setup

**One hub owns one db file at a time.** On startup a hub writes a PID lockfile
next to the db (`<db>.lock`). If a second hub tries to open the same `--path`
while the first is alive, it **refuses to start**:

```
database is already locked by ctxone-hub pid 12345 (lockfile: …/memory.db.lock);
refusing to start a second hub against the same db
```

This is deliberate — two hubs writing one SQLite file corrupts it (the
2026-04-28 incident). See [DATA_SAFETY.md](DATA_SAFETY.md). Everything below is
really about *who* holds that single lock.

## How a hub can run

`ctxone-hub` has two transports for talking to agents, plus the HTTP surfaces:

| Mode | Command | Serves | Owns the db |
|------|---------|--------|-------------|
| **MCP stdio** | `ctxone-hub --path DB` | MCP tools to the spawning agent | yes, while the agent runs |
| **HTTP (+Lens)** | `ctxone-hub --http --lens --path DB` | REST API + web UI + **MCP at `/mcp`** | yes, while the daemon runs |

By default `ctx init` configures agents to **spawn their own stdio hub**. That
agent's hub holds the lock, so you cannot also run an HTTP/Lens hub on the *same*
db at the same time.

> **The unified daemon (recommended).** An `--http` hub now also serves MCP at
> `/mcp` (Streamable HTTP), so one process covers MCP + REST + Lens and agents
> connect by URL instead of spawning their own child — no lockfile race,
> web UI always up, multiple agents share one graph. Configure it with
> `ctx init --transport http` (see [Topology B](#b-shared-http-daemon-mcp--rest--lens-recommended)).
> Design notes: [design/UNIFIED_HUB.md](design/UNIFIED_HUB.md).

## Choosing a topology

### A. Agent-only, no web UI (default, simplest)

Let the agent spawn and own its stdio hub. Nothing else touches the db. This is
what a fresh `ctx init` gives you.

- ✅ Zero extra processes, no port.
- ❌ No Lens; can't attach the CLI-over-HTTP to *this* db while the agent runs
  (the CLI reads the same graph, but only through a running HTTP hub).

### B. Shared HTTP daemon (MCP + REST + Lens) — recommended

Run **one** long-lived hub that serves everything, and point agents, the CLI, and
Lens at it. Nothing spawns its own child; there is exactly one db owner.

```bash
# 1. Run the one daemon (as a login service — see below — so it's up at boot).
ctxone-hub --http --lens --path ~/.ctxone/memory.db

# 2. Point the CLI at it.
export CTX_SERVER=http://localhost:3001

# 3. Configure your agents to connect by URL instead of spawning a hub.
ctx init --transport http                 # all detected tools
ctx init --transport http --tool claude   # or one tool
```

`ctx init --transport http` writes an MCP entry like:

```jsonc
{
  "mcpServers": {
    "ctxone": { "type": "http", "url": "http://localhost:3001/mcp?namespace=<detected>" }
  }
}
```

The `?namespace=` is filled in from the project detected in the current directory
(same scoping a per-project stdio hub would use). Override the endpoint with
`--mcp-url`, e.g. `--mcp-url http://mybox:3001/mcp`.

- ✅ One process, one lock, **startup order irrelevant** — the reboot race is gone.
  Lens always up, independent of any agent; multiple agents share one graph.
- MCP clients reach the daemon two ways, and `ctx init --transport http` writes
  the right one per client:
  - **Native URL** — Claude Code, Cursor, VS Code get `{"type":"http","url":…}`.
  - **`mcp-remote` bridge** — **Claude Desktop** has no native HTTP MCP support,
    so `ctx init` writes an stdio bridge
    (`npx -y mcp-remote <url> --transport http-only`) that proxies to the daemon.
    Requires Node/npx on PATH; a note is printed when this is used.
  - **Codex** — native HTTP via a `url` key in `config.toml`
    (`[mcp_servers.ctxone]\nurl = "…/mcp?namespace=…"`).
- ⚠️ A single daemon has no per-cwd project detection, so **one agent config maps
  to one namespace** (baked into the URL). An agent that roams many repos uses one
  fixed namespace or per-workspace configs. Need per-cwd auto-scoping? Use stdio
  (topology A) — it's still fully supported.

### C. Give an agent its own dedicated db (isolation)

Point one agent at a **different `--path`** so it never contends with your shared
hub or another agent. Two separate files → two separate locks → no conflict, and
two independent memory graphs.

Edit that agent's MCP config (or run `ctx init --tool <name>` after setting the
path). The stdio entry looks like:

```jsonc
// e.g. ~/Library/Application Support/Claude/claude_desktop_config.json
{
  "mcpServers": {
    "ctxone": {
      "command": "/opt/homebrew/bin/ctxone-hub",
      "args": ["--path", "/Users/user/.ctxone/claude.db", "--agent-id", "claude-desktop"]
    }
  }
}
```

Use this when:

- You want Claude's memory kept **separate** from a shared/team graph.
- Two agents on one machine should **not** see each other's memory.
- You're experimenting and don't want to touch your main graph.

> Isolated dbs do **not** sync. A fact remembered in `claude.db` is invisible to
> the hub serving `memory.db`. To browse an isolated db in Lens, run a hub on it
> at a **different port** while the agent is stopped, or on a copy:
> `ctxone-hub --http --lens --path /Users/user/.ctxone/claude.db --port 3002`.

### D. Browse a live agent's memory without fighting the lock

You can't open a second hub on a locked db. To look at the graph an agent is
actively using, snapshot it and serve the copy:

```bash
cp ~/.ctxone/memory.db /tmp/lens.db
ctxone-hub --http --lens --path /tmp/lens.db --port 3002
```

Read-only snapshot; it won't reflect writes after the copy. (The proper fix is
the shared daemon in topology B — one owner, Lens live on the same graph the
agents write.)

## Configuration reference

### Where the db lives

`ctx init` points agents at the canonical path:

- macOS / Linux: `~/.ctxone/memory.db`
- Windows: `%APPDATA%\ctxone\memory.db`

Override per agent with `--path` (topology C). A bare `ctxone-hub` with no
`--path` defaults to `./target/ctxone.db` (dev convenience) — always pass
`--path` in real setups.

### Useful flags

| Flag | Purpose |
|------|---------|
| `--path <PATH>` | SQLite db file (per-agent isolation lives here) |
| `--http` | Enable the REST API |
| `--lens` | Serve the web UI at `/` (requires `--http`) |
| `--port <N>` | HTTP port (default 3001) — use distinct ports for parallel dbs |
| `--agent-id <NAME>` | Attribution recorded on commits (`ctx blame`) |
| `--auth-token <TOK>` | Bearer token guarding REST + `/mcp` (env `CTXONE_AUTH_TOKEN`); loopback exempt |
| `--init` | Create the db file if missing (guards against path typos) |
| `--storage memory` | Ephemeral, no file, no lock — good for tests/demos |
| `--asd-path name=PATH` | Register an ASD code-graph repo for the process pool |

### Environment

| Var | Effect |
|-----|--------|
| `CTX_SERVER` | Hub URL the `ctx` CLI targets (default `http://localhost:3001`) |
| `CTX_NAMESPACE` | Project namespace for CLI/MCP scoping |
| `CTX_BRANCH` | Default memory branch/ref |
| `CTXONE_BACKUP_KEEP` | Startup snapshots to retain (default 5) |
| `CTXONE_BACKUP_INTERVAL_SECS` | Background VACUUM-INTO snapshot interval (default 1800; 0 disables) |
| `RUST_LOG` | Log level (`info` default) |

## Authentication

The hub binds `0.0.0.0` (all interfaces). With no token set, REST and `/mcp` are
reachable from the network **unauthenticated** — the hub logs a loud warning at
startup. Before exposing it beyond localhost, set a bearer token:

```bash
ctxone-hub --http --lens --path ~/.ctxone/memory.db --auth-token "$(openssl rand -hex 32)"
# or: CTXONE_AUTH_TOKEN=… ctxone-hub --http …
```

Behaviour:
- **Loopback peers (127.0.0.1/::1) are always exempt** — local CLI, Lens, and
  same-host agents keep working with no token.
- **Non-loopback requests must send** `Authorization: Bearer <token>` (else 401).
- Setting a token also relaxes `/mcp`'s loopback-only Host check so authenticated
  remote clients can connect (the bearer becomes the gate).

Clients — `ctx init --transport http` writes the token into each tool's config:
- **`ctx` CLI** — `--token <TOK>` or `CTX_TOKEN` (only needed for a remote hub).
- **`ctx init --transport http --auth-token <TOK>`** — embeds a literal bearer:
  a `headers` entry for native http (Claude Code/Cursor/VS Code) and a
  `--header` arg for the mcp-remote bridge (Claude Desktop). The token is written
  in plaintext into those config files (a warning is printed).
- **`ctx init --transport http --auth-token-env <VAR>`** — keeps the secret out
  of config files: Codex gets `bearer_token_env_var = "<VAR>"`, and native http
  entries reference `Bearer ${VAR}` (for clients that expand env vars). The
  mcp-remote bridge can't expand env vars, so Claude Desktop needs the literal
  `--auth-token` form.

> Residual note: loopback browser requests remain exempt and REST CORS is open
> (`allow_origin(Any)`), so the token is not same-host CSRF protection — a local
> web page can still reach the API. Tightening that is tracked separately.

## Running as a service

To make the HTTP/Lens hub survive reboots and start **before** any agent (so it,
not an agent's stdio child, owns the db), run it as a login service. `ctx service`
generates and registers the unit for you — launchd on macOS, a systemd **user**
unit on Linux:

```bash
ctx service install                       # lens on, port 3001, canonical db
ctx service install --port 3001 --path ~/.ctxone/memory.db
ctx service install --auth-token "$(openssl rand -hex 32)"   # embeds token (chmod 600)
ctx service install --dry-run             # print the unit + commands, write nothing
ctx service status                        # is it registered/running?
ctx service uninstall                     # stop + remove
```

The unit runs `ctxone-hub --http --lens --path <db> --port <port>` with
`RunAtLoad`/`KeepAlive` (macOS) or `Restart=on-failure` (systemd), logging to
`~/.ctxone/hub.log`. Preview it first with `--dry-run`. If another hub already
holds the db (an agent's stdio hub, or a manual one), the service will fail on
the lockfile — stop it first.

<details><summary>Equivalent hand-written launchd plist</summary>

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.ctxone.hub</string>
  <key>ProgramArguments</key>
  <array>
    <string>/opt/homebrew/bin/ctxone-hub</string>
    <string>--http</string>
    <string>--lens</string>
    <string>--path</string>
    <string>/Users/user/.ctxone/memory.db</string>
  </array>
  <key>KeepAlive</key><true/>
  <key>RunAtLoad</key><true/>
  <key>StandardErrorPath</key><string>/Users/user/.ctxone/hub.log</string>
</dict>
</plist>
```

```bash
launchctl load -w ~/Library/LaunchAgents/com.ctxone.hub.plist
```
</details>

> With the daemon running, point agents at it with `ctx init --transport http`
> and they stop spawning their own hub — one process, one owner, no race. Agents
> that can't do remote MCP must instead use a **separate db** (topology C) to
> avoid the lock collision. See [design/UNIFIED_HUB.md](design/UNIFIED_HUB.md).

## Troubleshooting

- **`database is already locked …`** — another hub owns that db. Find it
  (`ps aux | grep ctxone-hub`), or use a different `--path`/`--port`. Stale locks
  (owner gone) are reclaimed automatically.
- **Lens shows nothing / CLI can't connect** — check `CTX_SERVER` and that a hub
  is running with `--http`. `curl http://localhost:3001/api/health`.
- **Agent and Lens show different memory** — they're on different db paths
  (topology C). That's isolation working as configured.

See also: [ARCHITECTURE.md](ARCHITECTURE.md) · [DATA_SAFETY.md](DATA_SAFETY.md) ·
[TROUBLESHOOTING.md](TROUBLESHOOTING.md)

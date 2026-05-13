# ASD Code Intelligence Integration

CTXone integrates with
[AgentStateDeveloper (ASD)](https://github.com/agentstatelabs/AgentStateDeveloper)
to give AI agents structured, queryable access to your codebase —
symbols, call graphs, effects, and ledger decisions — alongside the
memory and planning tools they already use.

## What you get

When ASD is wired in, agents gain five new MCP tools:

| Tool | What it does |
|------|-------------|
| `code_repos` | List registered repos |
| `code_search` | Semantic search over symbols (concept, name, signature, file, ledger) |
| `code_read` | Full context for one symbol: signature, effects, ledger |
| `callers_of` | Inbound call graph edges |
| `callees_of` | Outbound call graph edges |

A typical agent workflow:

1. `code_search "payment processing"` → find relevant symbols
2. `code_read "payments.charge_card"` → see effects + ledger decisions
3. `callers_of "payments.charge_card"` → understand blast radius before editing

---

## Prerequisites

1. **ASD installed** — `asd` and `asd-serve` binaries on your `PATH`
2. **Repo indexed** — at least one `asd index <dir>` run against the codebase
3. **Hub started** with at least one `--asd-repo` or `--asd-url` flag

---

## Setup

### Option A — Hub-managed process pool (recommended)

The hub spawns `asd-serve` child processes on demand, one per repo.
Processes are killed after 5 minutes of idle and restarted transparently
on the next request. You never need to manage `asd-serve` yourself.

**Step 1: Index the codebase**

```bash
cd /home/user/myproject
asd init
asd index .
```

After indexing, ASD writes `myproject/.asd-state.db`. If `asd` is also
configured with a [repo registry](#asd-repo-registry), it auto-registers
this db so you can later switch between projects with `asd repo use`.

**Step 2: Start the hub with the repo**

```bash
ctxone-hub --http \
  --asd-repo myproject=/home/user/myproject/.asd-state.db
```

**Multiple repos:**

```bash
ctxone-hub --http \
  --asd-repo frontend=/home/user/frontend/.asd-state.db \
  --asd-repo backend=/home/user/backend/.asd-state.db \
  --asd-repo shared-lib=/home/user/shared-lib/.asd-state.db
```

### Option B — Pre-running asd-serve

If you prefer to control `asd-serve` yourself (e.g. for cluster deployments
or debugging):

```bash
# Start asd-serve for each repo
asd-serve --db /home/user/myproject/.asd-state.db &
# Prints: listening on 0.0.0.0:4120

# Register with the hub
ctxone-hub --http --asd-url myproject=http://127.0.0.1:4120
```

You can mix `--asd-repo` (pool-managed) and `--asd-url` (pre-running) in
the same hub invocation.

---

## ASD repo registry

ASD itself maintains a global registry at `~/.config/asd/repos.toml` that
tracks all indexed repos and which one is currently active. The `asd-mcp`
server reads this registry at startup and re-reads it when it changes, so
you can switch repos in a terminal without restarting the MCP server.

### Registry file location

```
~/.config/asd/repos.toml
```

### Registry file format

```toml
[repos]
myproject = "/home/user/myproject/.asd-state.db"
otherlib  = "/home/user/otherlib/.asd-state.db"
frontend  = "/home/user/frontend/.asd-state.db"

[active]
name = "myproject"
```

### Managing the registry

```bash
# Register a repo (run from the project directory, or pass --db)
asd repo add myproject
asd repo add otherlib --db /home/user/otherlib/.asd-state.db

# List all registered repos (* marks the active one)
asd repo list
# NAME                 ACTIVE DB PATH
# ─────────────────────────────────────────────────────────────────────────
# frontend                    /home/user/frontend/.asd-state.db
# myproject            *      /home/user/myproject/.asd-state.db
# otherlib                    /home/user/otherlib/.asd-state.db

# Switch the active repo
asd repo use otherlib

# Remove a repo
asd repo rm frontend
```

`asd index` auto-registers the db on every run — no separate `asd repo add`
needed after your first `asd index .`.

---

## Database resolution for asd-mcp

When the `asd-mcp` MCP server starts, it picks the database in this order:

1. `ASD_DB` environment variable (explicit override)
2. Active repo from `~/.config/asd/repos.toml`
3. `./.asd-state.db` in the current working directory

**Live switching:** `asd-mcp` polls `repos.toml` every 5 seconds. When
`asd repo use <name>` changes the active repo, the MCP server transparently
closes the old engine and opens the new one — agents don't need to
reconnect or restart.

This is only active when `ASD_DB` is not set. If you hard-wire `ASD_DB`,
the registry is ignored entirely.

---

## MCP config examples

### asd-mcp (direct ASD access)

Wire `asd-mcp` into Claude Code / Cursor for direct single-repo access:

```json
{
  "mcpServers": {
    "asd": {
      "command": "/path/to/asd-mcp"
    }
  }
}
```

`asd-mcp` resolves the db via the registry or `ASD_DB`. Run
`asd repo use <name>` in a terminal to switch repos; `asd-mcp` picks it
up within 5 seconds.

### ctxone-hub (memory + code intelligence together)

```json
{
  "mcpServers": {
    "ctxone": {
      "command": "/path/to/ctxone-hub",
      "args": [
        "--path", "/home/user/.ctxone/memory.db",
        "--asd-repo", "myproject=/home/user/myproject/.asd-state.db",
        "--asd-repo", "otherlib=/home/user/otherlib/.asd-state.db"
      ]
    }
  }
}
```

### Both simultaneously

Use both MCP servers at once — `asd` for code-specific tools, `ctxone` for
memory and planning:

```json
{
  "mcpServers": {
    "asd": {
      "command": "/path/to/asd-mcp"
    },
    "ctxone": {
      "command": "/path/to/ctxone-hub",
      "args": ["--path", "/home/user/.ctxone/memory.db"]
    }
  }
}
```

---

## HTTP proxy routes

When the hub is started with `--http`, the code intelligence proxy is also
available over REST:

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/code` | List registered ASD repos |
| `GET` | `/api/code/{repo}/{*path}` | Forward to `asd-serve /api/v1/{path}` |
| `POST` | `/api/code/{repo}/{*path}` | Forward POST |

Examples:

```bash
# List repos
curl http://localhost:3001/api/code

# Search symbols
curl "http://localhost:3001/api/code/myproject/search?q=charge+card"

# Read a symbol
curl "http://localhost:3001/api/code/myproject/symbols/payments.charge_card"

# Call graph
curl "http://localhost:3001/api/code/myproject/symbols/payments.charge_card/callers"
```

The Lens UI (`ctxone-hub --lens`) uses these routes to display the code
intelligence panel.

---

## Multi-repo agent workflow

When multiple repos are registered, agents select a repo by name:

```
Agent: code_search "authentication middleware" repo=backend
→ [{ qname: "auth.verify_token", ... }, ...]

Agent: code_read "auth.verify_token" repo=backend
→ { symbol: ..., effects: ..., ledger: [...] }

Agent: callers_of "auth.verify_token" repo=backend
→ [{ qname: "api.handle_request", ... }]
```

When only one repo is registered, the `repo` parameter can be omitted and
the single registered repo is used automatically.

---

## Troubleshooting

**`code_repos` returns an empty list**

The hub was started without `--asd-repo` or `--asd-url`. Restart with at
least one repo configured.

**`error: repo 'myproject' is not registered in the pool`**

The name you passed to `repo` doesn't match any `--asd-repo` or `--asd-url`
name. Run `code_repos` to see the registered names.

**`error: timed out waiting for asd-serve to start`**

`asd-serve` isn't on `$PATH`, or the db file doesn't exist. Check that
`which asd-serve` returns the right binary and that the db path is correct.

**asd-mcp keeps using the wrong repo**

- `ASD_DB` is set and overrides the registry. Unset it if you want registry
  control.
- The watcher checks mtime every 5 s — wait a few seconds after `asd repo use`.

---

## See also

- [MCP_TOOLS.md](MCP_TOOLS.md) — full tool reference including code tools
- [CLI_REFERENCE.md](CLI_REFERENCE.md) — `ctx serve` flags and `ctxone-hub` options
- [ASD documentation](https://docs.agentstatelabs.com/asd/) — indexing, ledger, effects

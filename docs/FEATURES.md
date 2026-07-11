# CTXone Features & Command Reference

A complete tour of what CTXone does and every `ctx` command it exposes. For
a narrative "how do I actually use this" guide, read
[WALKTHROUGH.md](WALKTHROUGH.md). For exhaustive per-flag detail see
[CLI_REFERENCE.md](CLI_REFERENCE.md); for the agent-facing tools see
[MCP_TOOLS.md](MCP_TOOLS.md) and [HTTP_API.md](HTTP_API.md).

CTXone has four surfaces:

- **CTXone Hub** (`server/`) — MCP server (52 tools) + HTTP API; the memory
  interface for AI tools.
- **CTXone Engine** (`engine/`) — the core memory + graph layer
  (AgentStateGraph).
- **CTXone Lens** (`web/`) — the web UI: dashboard, plans, sessions, browse,
  history, branches, taint, diff.
- **`ctx`** (`cli/`) — the command line (everything below).

---

## The primitives

| Primitive | What it is |
|---|---|
| **Memory graph** | A content-addressed graph of facts, decisions, and context stored under paths (`/memory/facts/…`, `/sessions/…`). Recall ranks by relevance within a token budget instead of dumping everything — O(log n), not flat. |
| **Pinned vs primed** | *Pinned* memories are always included (critical context); *primed* memories are ranked candidates that recall pulls in when relevant. |
| **Plans** | A container of tasks that survives across sessions — the cure for "plan rot," where markdown todos drift from reality the moment work starts. |
| **Branches** | Memory is branchable like git: sandbox speculative work on its own branch, then diff and merge it back. |
| **Taint** | Markers that flag paths as needing verification (`taint`), blocking writes (`quarantine`), or watch-only (`watch`). |
| **Token accounting** | Every response reports tokens sent vs the flat-memory baseline, making the savings measurable and provable. |
| **Durability** | Automatic SQLite snapshots, a PID lockfile, an inode-drift watchdog, and `ctx db backup`/`restore` — the memory db is the crown jewel. |

---

## Commands

### Setup & wiring

| Command | What it does |
|---|---|
| `ctx init` | Auto-detect and configure your AI tools with the CTXone MCP server. `--transport http` (default — point tools at a shared daemon by URL) or `--transport stdio` (each tool spawns its own hub); `--mcp-url`, `--auth-token`/`--auth-token-env` to embed a bearer; `--tool claude/cursor/vscode/codex/gemini/grok`, `--global` vs `--project`, `--config-path`, `--dry-run`. |
| `ctx service install/uninstall/status` | Install the Hub as a login/boot service (launchd on macOS, systemd user unit on Linux, Task Scheduler on Windows) so the daemon owns the db before any tool starts. `--port`, `--path`, `--no-lens`, `--auth-token`, `--dry-run`. |
| `ctx agents install/show` | Manage `AGENTS.md` — a short pinned document that teaches AI tools how to use CTXone. Prompts before writing unless `--yes`. |
| `ctx skill` | Install CTXone's **Agent Skill** (`SKILL.md`) into each host's skills directory. Version-stamped. Installs the combined **CTXone + ASD** suite skill when the `asd` CLI is present. `--status` / `--dry-run` / `--project` / `--tool` / `--remove`. |
| `ctx bootstrap` | Print a paste-into-your-agent block that installs + primes CTXone — and offers to set up AgentStateDeveloper too. |
| `ctx serve` | Start the CTXone Hub. `--http` serves the REST API **and MCP at `/mcp`** (Streamable HTTP); `--lens` adds the web UI — one process for all three. `--port`, `--storage sqlite/postgres/memory`, `--path <db>`, `--auth-token` (bearer for non-loopback), `--allowed-origin` (extra browser origins). |
| `ctx doctor` | End-to-end health checks with suggested fixes (inode drift, stray db files, missing recent backups). |
| `ctx config` | Read/write persistent defaults in `~/.ctxone/config.toml`. |
| `ctx completion <shell>` | Generate a shell completion script. |

### Memory

| Command | What it does |
|---|---|
| `ctx remember <fact>` | Store a fact. `--importance high/medium/low`, `--context <category>`, `--tags`. |
| `ctx recall <topic>` | Retrieve the most relevant memories for a topic within a `--budget` (tokens). `--exact` re-tokenizes with tiktoken for precise counts. |
| `ctx context <project>` | Load full context for a project. |
| `ctx prime <file>` | Load a markdown file as primed memory; `--pin` makes it always-included. |
| `ctx pinned` | List pinned (always-included) memories. |
| `ctx forget <path>` | Delete a memory at a path; `--reason` shows up in blame. |
| `ctx search <query>` | Literal substring search over the graph — full matching paths and values, no budget (unlike recall). |
| `ctx ls [prefix]` / `ctx get <path>` | List paths under a prefix / read the value at a path. |
| `ctx tokens [text]` | Count exact tokens (tiktoken cl100k_base); reads stdin if no argument. |

### Plans

| Command | What it does |
|---|---|
| `ctx plan new/add/next/done/…` | Manage plans: create a plan, add tasks, ask "what's next?", and mark tasks done with proof (`--proof commit:abc1234`). Plans persist across sessions and stay anchored to reality. |

### Branches & history

| Command | What it does |
|---|---|
| `ctx branch <name>` / `ctx branches` | Create a memory branch (from `--from`, default `main`) / list branches. Use `ctx --branch <name> …` to scope any command. |
| `ctx diff <ref_a> <ref_b>` | Diff two refs (branches, tags, commits). |
| `ctx merge <source>` | Merge a source branch into `--into` (default `main`), `-m` message. |
| `ctx log` / `ctx blame <path>` / `ctx tail` | Recent commit history / provenance chain for a path (who wrote it, when, why) / live `tail -f` monitor of new commits. |
| `ctx why-did-we <decision>` | Trace decision provenance — find facts/decisions mentioning a phrase and return the blame chain. Run this **before** reversing a settled decision (security/licensing/deployment). |

### Taint (verification gates)

| Command | What it does |
|---|---|
| `ctx taint list/check/…` | List taints (filter by path prefix, kind, resolved-status) or check whether a write would be allowed at a path for an agent at a given confidence (read-only). |

### Sessions & token accounting

| Command | What it does |
|---|---|
| `ctx status` / `ctx stats` | Hub status and connection info / token-savings statistics. |
| `ctx session metrics` | Token usage, cost, and cache metrics for Claude Code sessions (parses `~/.claude/projects/` transcripts). `--all`, `--json`, `--gap`, `--verbose`. |
| `ctx ingest-session` | Import a Claude Code session JSONL into memory — extracts structured memories + token usage. Needs `ANTHROPIC_API_KEY`. |
| `ctx capture-turn` | Capture memories + token usage from the last agent turn. Called automatically by the Claude Code / Codex Stop hook. |
| `ctx summarize-session` | Capture key points + decisions for the current/named session (`--point`, `--decision`). |
| `ctx record-usage` | Report LLM token usage for the current session (`--input`, `--output`, `--cache-read`, `--cache-create`, `--model`, `--provider`). |
| `ctx demo` | Seed the Hub with realistic demo data and show live token savings. |

### Database admin

| Command | What it does |
|---|---|
| `ctx db backup/restore` | Snapshot the live db (SQLite `VACUUM INTO`, consistent against a running hub) or restore from a snapshot (hub must be stopped). |

---

## MCP tools & HTTP API

The Hub exposes **52 MCP tools** to agents — memory (`remember`, `recall`,
`context`), plans, branches, taint (`taint_*`), provenance (`why_did_we`,
`summarize_session`), and token accounting (`record_llm_usage`). Full list:
[MCP_TOOLS.md](MCP_TOOLS.md). The REST surface is in
[HTTP_API.md](HTTP_API.md).

Those MCP tools are served two ways: over **stdio** (a per-tool child), and —
this is the standard setup — over **HTTP at `/mcp`** (Streamable HTTP) by an
`--http` daemon, so one process serves MCP + REST + Lens and every tool connects
by URL. Guard a network-exposed hub with a bearer token (`--auth-token`;
loopback is exempt) and, if needed, browser origins (`--allowed-origin`). See
[DEPLOYMENT.md](DEPLOYMENT.md).

---

## Editions

Everything above is **OSS** — self-hosted, no account. Team (shared team
Hub) and Enterprise (multi-tenancy, RBAC/SSO, audit) build on top. See
[LICENSING.md](../LICENSING.md).

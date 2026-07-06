# CTXone

Persistent, searchable, accountable memory for AI agents. Eliminate context anxiety.

**Part of a suite:** CTXone is the shared **team layer** for
**[AgentStateDeveloper](https://github.com/agentstatelabs/AgentStateDeveloper)**
(per-developer code context). Installing either offers the other — see
[Pairs with AgentStateDeveloper](#pairs-with-agentstatedeveloper).

## Components

| Component | Directory | Description |
|-----------|-----------|-------------|
| **CTXone Hub** | `server/` | MCP server (52 tools) + HTTP API — the memory interface for AI tools |
| **CTXone Engine** | `engine/` | Core memory + graph layer (AgentStateGraph) |
| **CTXone Lens** | `web/` | Web UI: dashboard, plans, sessions, browse, history, branches, taint, diff. ⌘K palette, 15s auto-refresh, multi-theme |
| **ctx** | `cli/` | CLI for memory, plans, branches, taint, and team operations |
| **ctxone (Python)** | `bindings/python/` | Python client library (`pip install ctxone`) |

## Pairs with AgentStateDeveloper

CTXone is the **team layer** for
**[AgentStateDeveloper](https://github.com/agentstatelabs/AgentStateDeveloper)** —
they're built as a suite:

- **ASD** — per-developer **code context**: decision ledger, effect
  declarations, call graph, and impact analysis.
- **CTXone** — **shared team memory**: decisions, plans, and context that travel
  across the whole team.

Each works standalone, but together:

- Installing either one **offers to set up the other** — a one-time, dismissable
  nudge (suppress with `--no-nudge` or `CTX_NO_SUGGEST=1`).
- When both are installed, `ctx skill` also installs a **combined suite skill**
  that teaches the agent the joint workflow: use ASD for the code specifics, and
  record what you decide into CTXone so the team inherits it.
- `ctx bootstrap` offers to install **both**.

## Quick Start

**See the [5-minute quickstart](docs/QUICKSTART.md)** — from nothing to live
token savings in 5 minutes.

### Install

**macOS (Homebrew):**

```bash
brew install ctxone/tap/ctxone
```

**macOS / Linux** (one-liner):

```bash
curl -sSL https://raw.githubusercontent.com/ctxone/ctxone/main/install.sh | sh
```

**Uninstall:**

```bash
curl -sSL https://raw.githubusercontent.com/ctxone/ctxone/main/uninstall.sh | sh
```

**Windows** (PowerShell, one-liner):

```powershell
iwr https://raw.githubusercontent.com/ctxone/ctxone/main/install.ps1 | iex
```

Full Windows guide with background service setup, AI tool paths,
updates, and troubleshooting: [docs/WINDOWS.md](docs/WINDOWS.md).

**Docker** (any platform — image is multi-arch `linux/amd64` + `linux/arm64`):

```bash
docker run -p 3001:3001 -v ctxone-data:/data ghcr.io/ctxone/ctxone:latest
```

This works identically on macOS (via Docker Desktop), Linux (native),
and Windows (via Docker Desktop's WSL2 backend — the Linux image runs
inside WSL2 and Windows sees port 3001 on localhost). There's no
separate "Windows container" image; Windows users can either run the
Linux image under Docker Desktop or use `install.ps1` for native
`ctx.exe` / `ctxone-hub.exe`.

**Python**:

```bash
pip install ctxone
```

**From source**:

```bash
git clone --recursive https://github.com/ctxone/ctxone.git
cd ctxone
cargo build --workspace --release
cd web && npm install && npm run build
```

## Setup

Wire CTXone into your coding agent. The fastest path is to let the agent do it.

### Paste-to-your-agent (recommended)

```bash
ctx bootstrap
```

Prints a block you paste into whatever agent you're already in; it installs and
primes CTXone — and offers to set up **AgentStateDeveloper** (code context) too.

### Individual commands

| Command | Sets up |
|---|---|
| `ctx init` | Auto-detect and configure your AI tools (MCP). Target one with `ctx init --tool claude` / `--tool cursor`. |
| `ctx agents install` | Write the shared `AGENTS.md` and prime it as pinned memory in the Hub. |
| `ctx skill` | Install CTXone's **Agent Skill** (`SKILL.md`) into each host's skills directory — teaches the agent to record decisions, plans, and memory. Version-stamped. When the `asd` CLI is present, it also installs the combined **CTXone + ASD** suite skill. |

```bash
ctx skill --status    # what's installed, per host
ctx skill --dry-run   # preview without writing
```

## Usage

```bash
# Store a fact
ctx remember "We use BSL-1.1 for all projects" --importance high --context licensing

# Retrieve relevant memories
ctx recall "licensing decisions"

# Load full project context
ctx context myproject

# Track multi-step work across sessions
ctx plan new my-feature
ctx plan add my-feature "Wire up new endpoint"
ctx plan next my-feature        # what should I do next?
ctx plan done my-feature t-001 --proof commit:abc1234

# Sandbox speculative work on its own branch (memory follows the branch)
ctx --branch feature/x remember "API renamed from foo to bar"

# Check Hub status and token savings
ctx status
ctx stats
```

## Token Savings

CTXone tracks token usage in real-time. Every response includes how many tokens were
sent vs how many would have been sent with flat memory loading — making the savings
measurable and provable.

## Durability

Your memory db is the only thing in CTXone that can't be regenerated, so
the hub treats it as the crown jewel. All defenses are on by default:

- **Automatic snapshots** via SQLite `VACUUM INTO` — one on startup and
  one every 30min (configurable: `CTXONE_BACKUP_INTERVAL_SECS`,
  `CTXONE_BACKUP_KEEP`). Stored next to the db as `<db>.bak.<utc>`.
- **PID lockfile** — a second hub against the same db refuses to start
  with a clear error instead of silently corrupting writes.
- **Inode-drift watchdog** — if the db file is `rm`'d or replaced under
  a running hub, you get a WARN within 30s instead of silent loss at
  next restart.
- **Strict argv** — `ctxone-hub --version` and `--help` short-circuit
  before any storage code runs, and a missing db file is only created
  when you pass `--init`. Stray invocations no longer leave debris.
- **Recovery** — `ctx db backup` triggers a snapshot on demand;
  `ctx db restore <snapshot>` swaps one back in (current db is
  preserved at `<db>.pre-restore-<ts>` first).
- **`ctx doctor`** flags inode drift, stray db files, and missing
  recent backups, with one-line fixes.

Full details: [Data Safety](docs/DATA_SAFETY.md).

## Architecture

```
ctxone/
├── cli/           # ctx CLI (Rust)
├── server/        # CTXone Hub — MCP server (Rust)
├── engine/        # AgentStateGraph core (git submodule)
├── web/           # CTXone Lens — web UI (SvelteKit)
├── docs/          # Product strategy and design docs
├── install.sh
├── Dockerfile
└── docker-compose.yml
```

## Documentation

**Get started:**
- [Quickstart](docs/QUICKSTART.md) — from nothing to live token savings in 5 minutes
- [Walkthrough](docs/WALKTHROUGH.md) — install → daily loop → what happens under the covers → using CTXone + ASD together
- [Windows guide](docs/WINDOWS.md) — full install, background service, and troubleshooting for Windows
- [Architecture](docs/ARCHITECTURE.md) — the mental model (pinned vs primed, how recall ranks, why O(log n))
- [Token Savings](docs/TOKEN_SAVINGS.md) — how the ratio is computed, how to read it, how to maximize it
- [Cookbook](docs/COOKBOOK.md) — git hooks, cron jobs, shell prompts, team setups
- [Data Safety](docs/DATA_SAFETY.md) — snapshots, lockfile, watchdog, and `ctx db backup`/`restore`

**Reference:**
- [Features & Command Reference](docs/FEATURES.md) — what CTXone does, grouped by capability
- [CLI Reference](docs/CLI_REFERENCE.md) — every `ctx` command, flag, and exit code
- [HTTP API](docs/HTTP_API.md) — REST endpoints exposed by the Hub
- [MCP Tools](docs/MCP_TOOLS.md) — MCP tools exposed to agents
- [Integrations](docs/INTEGRATIONS.md) — wiring into Claude Code, Cursor, VS Code, Codex
- [Open WebUI](docs/OPENWEBUI.md) — native Tool + Filter plugins for Open WebUI
- [Troubleshooting](docs/TROUBLESHOOTING.md) — top 10 errors and fixes

**Runnable examples:** see [examples/](examples/)

**Strategy and background:**
- [Product Vision](docs/VISION.md)
- [Context Anxiety](docs/CONTEXT_ANXIETY.md) — the problem we solve
- [Token Economics](docs/TOKEN_ECONOMICS.md) — the math behind the savings ratio
- [Use Cases](docs/USE_CASES.md)
- [Memory MCP Design](docs/MEMORY_MCP_DESIGN.md) — technical design

## License & editions

CTXone is **open source, commercially supported**, and ships in three editions:

- **OSS** (this repo) — the full self-hosted Hub: memory, plans, recall,
  branches, token-savings accounting, and Lens UI. No account required.
- **Team** — a shared team Hub as the collaboration layer (shared memory,
  plans, and decisions across the team), paired with
  **[AgentStateDeveloper](https://github.com/agentstatelabs/AgentStateDeveloper)**
  as the suite's code-context half.
- **Enterprise** — org-scale operation: multi-tenancy, RBAC/SSO, audit, and
  compliance controls.

The code is licensed under the **Business Source License 1.1 (BSL 1.1)**.
**You can** use CTXone in production, self-host it, modify it, and build on top
of it — all without a commercial license. **You cannot** offer CTXone itself as
a competing managed service or redistribute it inside a product you sell. Each
version converts to **Apache License 2.0** four years after release.

Full plain-English summary and the edition breakdown:
[LICENSING.md](LICENSING.md) ([LICENSE](LICENSE) is the legal text).
Team/Enterprise or commercial questions:
[licensing@agentstatelabs.com](mailto:licensing@agentstatelabs.com).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, project structure,
and how to get involved. All contributors are expected to follow the
[Code of Conduct](CODE_OF_CONDUCT.md).

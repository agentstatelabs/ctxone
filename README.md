# CtxOne

Persistent, searchable, accountable memory for AI agents. Eliminate context anxiety.

## Components

| Component | Directory | Description |
|-----------|-----------|-------------|
| **CtxOne Hub** | `server/` | MCP server — the memory interface for AI tools |
| **CtxOne Engine** | `engine/` | Core memory + graph layer (AgentStateGraph) |
| **CtxOne Lens** | `web/` | Web UI for browsing agent memory |
| **ctx** | `cli/` | CLI for interacting with agent memory |
| **ctxone (Python)** | `bindings/python/` | Python client library (`pip install ctxone`) |

## Quick Start

**See the [5-minute quickstart](docs/QUICKSTART.md)** — from nothing to live
token savings in 5 minutes.

### Install (curl)

```bash
curl -sSL https://raw.githubusercontent.com/ctxone/ctxone/main/install.sh | sh
```

### Install (Docker)

```bash
docker compose up -d
```

### Install (from source)

```bash
git clone --recursive https://github.com/ctxone/ctxone.git
cd ctxone
cargo build --workspace --release
cd web && npm install && npm run build
```

## Setup

```bash
# Auto-detect and configure your AI tools
ctx init

# Or target a specific tool
ctx init --tool claude
ctx init --tool cursor
```

## Usage

```bash
# Store a fact
ctx remember "We use BSL-1.1 for all projects" --importance high --context licensing

# Retrieve relevant memories
ctx recall "licensing decisions"

# Load full project context
ctx context myproject

# Check Hub status and token savings
ctx status
ctx stats
```

## Token Savings

CtxOne tracks token usage in real-time. Every response includes how many tokens were
sent vs how many would have been sent with flat memory loading — making the savings
measurable and provable.

## Architecture

```
ctxone/
├── cli/           # ctx CLI (Rust)
├── server/        # CtxOne Hub — MCP server (Rust)
├── engine/        # AgentStateGraph core (git submodule)
├── web/           # CtxOne Lens — web UI (SvelteKit)
├── docs/          # Product strategy and design docs
├── install.sh
├── Dockerfile
└── docker-compose.yml
```

## Documentation

**Get started:**
- [Quickstart](docs/QUICKSTART.md) — from nothing to live token savings in 5 minutes
- [Architecture](docs/ARCHITECTURE.md) — the mental model (pinned vs primed, how recall ranks, why O(log n))
- [Token Savings](docs/TOKEN_SAVINGS.md) — how the ratio is computed, how to read it, how to maximize it
- [Cookbook](docs/COOKBOOK.md) — git hooks, cron jobs, shell prompts, team setups

**Reference:**
- [CLI Reference](docs/CLI_REFERENCE.md) — every `ctx` command, flag, and exit code
- [HTTP API](docs/HTTP_API.md) — REST endpoints exposed by the Hub
- [MCP Tools](docs/MCP_TOOLS.md) — MCP tools exposed to agents
- [Integrations](docs/INTEGRATIONS.md) — wiring into Claude Code, Cursor, VS Code, Codex
- [Troubleshooting](docs/TROUBLESHOOTING.md) — top 10 errors and fixes

**Runnable examples:** see [examples/](examples/)

**Strategy and background:**
- [Product Vision](docs/VISION.md)
- [Context Anxiety](docs/CONTEXT_ANXIETY.md) — the problem we solve
- [Token Economics](docs/TOKEN_ECONOMICS.md) — the math behind 60x savings
- [Use Cases](docs/USE_CASES.md)
- [Memory MCP Design](docs/MEMORY_MCP_DESIGN.md) — technical design

## License

CtxOne is licensed under the **Business Source License 1.1 (BSL 1.1)**.

**You can** use CtxOne in production, embed it in your products, self-host it on
your own infrastructure, modify it, and build commercial products on top of it —
all without a commercial license.

**You cannot** offer CtxOne itself as a competing commercial managed service.

Each version automatically converts to **Apache License 2.0** four years after
release. See [LICENSING.md](LICENSING.md) for the full plain-English summary and
[LICENSE](LICENSE) for the legal text.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, project structure,
and how to get involved. All contributors are expected to follow the
[Code of Conduct](CODE_OF_CONDUCT.md).

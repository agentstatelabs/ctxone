# CTXone

Persistent, searchable, accountable memory for AI agents. Eliminate context anxiety.

## Components

| Component | Directory | Description |
|-----------|-----------|-------------|
| **CTXone Hub** | `server/` | MCP server (31 tools) + HTTP API — the memory interface for AI tools |
| **CTXone Engine** | `engine/` | Core memory + graph layer (AgentStateGraph) |
| **CTXone Lens** | `web/` | Web UI: dashboard, plans, sessions, browse, history, branches, taint, diff. ⌘K palette, 15s auto-refresh, multi-theme |
| **ctx** | `cli/` | CLI for memory, plans, branches, taint, and team operations |
| **ctxone (Python)** | `bindings/python/` | Python client library (`pip install ctxone`) |

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
- [Windows guide](docs/WINDOWS.md) — full install, background service, and troubleshooting for Windows
- [Architecture](docs/ARCHITECTURE.md) — the mental model (pinned vs primed, how recall ranks, why O(log n))
- [Token Savings](docs/TOKEN_SAVINGS.md) — how the ratio is computed, how to read it, how to maximize it
- [Cookbook](docs/COOKBOOK.md) — git hooks, cron jobs, shell prompts, team setups

**Reference:**
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

## License

CTXone is licensed under the **Business Source License 1.1 (BSL 1.1)**.

**You can** use CTXone in production, embed it in your products, self-host it on
your own infrastructure, modify it, and build commercial products on top of it —
all without a commercial license.

**You cannot** offer CTXone itself as a competing commercial managed service.

Each version automatically converts to **Apache License 2.0** four years after
release. See [LICENSING.md](LICENSING.md) for the full plain-English summary and
[LICENSE](LICENSE) for the legal text.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, project structure,
and how to get involved. All contributors are expected to follow the
[Code of Conduct](CODE_OF_CONDUCT.md).

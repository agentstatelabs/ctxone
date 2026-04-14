# CtxOne

Persistent, searchable, accountable memory for AI agents. Eliminate context anxiety.

## Components

| Component | Directory | Description |
|-----------|-----------|-------------|
| **CtxOne Hub** | `server/` | MCP server — the memory interface for AI tools |
| **CtxOne Engine** | `engine/` | Core memory + graph layer (AgentStateGraph) |
| **CtxOne Lens** | `web/` | Web UI for browsing agent memory |
| **ctx** | `cli/` | CLI for interacting with agent memory |

## Quick Start

### Install (curl)

```bash
curl -sSL https://ctxone.dev/install.sh | sh
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

- [Product Vision](docs/VISION.md)
- [Context Anxiety](docs/CONTEXT_ANXIETY.md) — the problem we solve
- [Token Economics](docs/TOKEN_ECONOMICS.md) — the math behind 60x savings
- [Use Cases](docs/USE_CASES.md)
- [Memory MCP Design](docs/MEMORY_MCP_DESIGN.md) — technical design

## License

BSL-1.1

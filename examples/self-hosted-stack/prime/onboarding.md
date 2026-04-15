# Team conventions

We default to BSL-1.1 for new repos. Every release auto-converts to
Apache-2.0 four years after the release date. Do not add an MIT or
GPL file unless a customer contract requires it.

# Architecture

CtxOne is a memory layer wrapping AgentStateGraph. It runs as an HTTP
server on port 3001, stores data in SQLite by default, and exposes
tools over MCP for AI coding assistants. Lens is a SvelteKit web UI
that hits the Hub's REST API.

# Local setup

Run `docker compose up` from `examples/self-hosted-stack/`. The stack
includes Ollama and Open WebUI so you have a full self-hosted AI
environment in one command. Open http://localhost:8080 for chat and
http://localhost:5173 for the memory browser.

# Common operations

To prime a new project's README as pinned context, run
`ctx prime --source <project> --pinned README.md`. To see what's
currently pinned, use `ctx ls /memory/pinned` or open Lens.

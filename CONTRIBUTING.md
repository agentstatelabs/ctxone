# Contributing to CtxOne

Thank you for your interest in CtxOne! This project is building a persistent memory layer for AI agents — the thing that eliminates context anxiety and makes every session start smart. Contributions are welcome.

## How this project is developed

CtxOne is developed on a private GitLab instance and **mirrored, read-only, to GitHub**. GitHub is the public home — it's where you file issues and open pull requests, and it always reflects the current `main` and release tags — but the canonical history lives on GitLab.

One consequence matters for contributors: **GitHub's `main` is force-advanced from GitLab on every change, so pull requests are never merged with the GitHub "Merge" button** (that would be overwritten on the next sync). Instead, accepted changes are applied on the GitLab side by the project owner and then re-published to GitHub. Your commits and authorship are preserved, and the PR is closed with a link to the landed commit. If your merge doesn't come from the GitHub button, that's the mirror model working — not a rejection.

## Getting Started

1. **Fork and clone** the repository (with submodules)
   ```bash
   git clone --recursive https://github.com/AgentStateLabs/CTXone.git
   cd ctxone
   ```
2. **Install Rust**: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
3. **Install Node** (for the web UI): any recent LTS
4. **Build the Rust workspace**: `cargo build --workspace`
5. **Install web dependencies**: `cd web && npm install`
6. **Try the CLI**: `./target/debug/ctx --help`

## Project Structure

```
ctxone/
├── cli/           # ctx CLI (Rust, clap)
├── server/        # CtxOne Hub — MCP server (Rust, rmcp + axum)
├── engine/        # AgentStateGraph core (git submodule)
├── web/           # CtxOne Lens — web UI (SvelteKit)
├── docs/          # Product strategy and design docs
├── install.sh
├── Dockerfile
└── docker-compose.yml
```

Read `docs/VISION.md` first to understand the product direction, then `docs/MEMORY_MCP_DESIGN.md` for the technical design of the 6 memory tools.

## Components

- **CtxOne Hub** (`server/`) — MCP server exposing `remember`, `recall`, `context`, `summarize_session`, `what_changed_since`, `why_did_we`. Wraps AgentStateGraph primitives into memory-oriented operations and tracks token savings.
- **CtxOne Engine** (`engine/`) — AgentStateGraph as a git submodule. This is the persistent, versioned, branchable state store underneath everything. Changes to the engine go upstream to the AgentStateGraph repo.
- **CtxOne Lens** (`web/`) — SvelteKit web app for browsing the memory graph: dashboard, path browser, search, commit history.
- **ctx** (`cli/`) — Rust CLI for interacting with the Hub over HTTP, plus `ctx init` which auto-configures AI tools (Claude Code, Cursor, VS Code, Codex, Gemini).

## How to Contribute

### Good First Issues

Look for issues labeled `good-first-issue`:

- **CLI polish** — better error messages, shell completions, `ctx --version`
- **Lens pages** — flesh out dashboard with token savings chart, improve browse tree
- **Install experience** — test `install.sh` on fresh machines, fix edge cases
- **Docs** — tutorials, screenshots, demo recordings

### Medium Issues

- **HTTP API for Hub** — the Hub currently only runs as stdio MCP; add an HTTP mode with routes for memory tools and `/api/stats/tokens`
- **More AI tool support in `ctx init`** — Gemini CLI, Windsurf, full Codex TOML support
- **Per-session token budgets** — let callers specify a hard budget for `recall` and enforce it

### Larger Contributions

- **SDK crates/packages** — `sdk/` Rust crate + TypeScript/Python packages for programmatic use
- **ThreadWeaver integration** — wire the Hub into the chat app so "close a conversation, open a new one, context preserved" works end to end
- **Multi-tenant Hub** — team-shared memory with access control on branches

## Development Workflow

1. Create a branch: `git checkout -b feature/my-change`
2. Make changes
3. Run Rust tests: `cargo test --workspace`
4. Run formatter: `cargo fmt`
5. Run clippy: `cargo clippy --workspace`
6. For web changes: `cd web && npm run check && npm run build`
7. Commit with a clear message describing what and why
8. Open a focused, single-purpose pull request against `main` on GitHub

**Review and merge.** A maintainer reviews the PR. All changes are merged by the **project owner**, who applies the change on GitLab; the mirror then brings it to GitHub and the PR is closed as landed — the merge won't come from the GitHub button.

## Code Style

- Follow standard Rust conventions
- Write doc comments for public items
- Add tests for new functionality
- Keep the Hub thin — push logic into the engine (AgentStateGraph) when it belongs there
- Engine changes go to the AgentStateGraph repo, not CtxOne

## Architecture Principles

- **Memory is the product.** The CLI, Lens, and Hub are all surfaces over the same memory graph. Keep them consistent.
- **Token savings are measurable.** Every Hub response includes `_ctxone_stats`. Don't add operations that skip this tracking.
- **Frictionless install is non-negotiable.** If a change makes the install story harder, we need a very good reason.
- **The engine stays generic.** Memory-specific logic lives in `server/`, not in the AgentStateGraph submodule.

## Licensing

CtxOne is licensed under the Business Source License 1.1. By contributing, you agree that your contributions will be licensed under the same terms. See `LICENSING.md` for the plain-English summary.

## Questions?

Open an issue or start a discussion. We're building the memory layer every AI agent needs — your perspective matters.

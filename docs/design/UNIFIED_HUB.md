# Design: One Hub, All Surfaces (MCP-over-HTTP)

**Status:** ✅ Implemented (server + `ctx init --transport http`).
**Goal:** A single long-lived `ctxone-hub` process that serves **MCP + REST + Lens
web UI** from one port, so startup order never matters and there is exactly one
owner of the db.

**What shipped:** `ctxone-hub --http` mounts the MCP tool surface at `/mcp`
(Streamable HTTP), scoped per request by `?namespace=` (see the spike note
below). `ctx init --transport http [--mcp-url …]` writes a
`{"type":"http","url":…}` client entry with the detected namespace baked in.
stdio remains the default and a fully-supported fallback. See
[../DEPLOYMENT.md](../DEPLOYMENT.md) for operator setup.

**Spike finding (informs the design):** rmcp 1.7's `StreamableHttpService` takes a
*context-free* factory (`Fn() -> Result<S>`), so the namespace cannot be read
inside it. We therefore dispatch at the axum layer — one `StreamableHttpService`
per namespace, cached, each factory capturing a repo already forked (and
`init()`-ed) to that namespace. Implemented in `server/src/mcp_http.rs`.

## Problem

Today the hub has two mutually-exclusive modes ([server/src/main.rs](../../server/src/main.rs)):

```rust
if http_mode { /* REST (+Lens) */ } else { /* MCP over stdio */ }
```

MCP is served **only over stdio**, so every agent (Claude Desktop, Cursor, …)
*spawns its own* `ctxone-hub` child that opens `~/.ctxone/memory.db` directly.
That child takes an exclusive **PID lockfile** (`<db>.lock`,
[server/src/lockfile.rs](../../server/src/lockfile.rs)) — added after the
2026-04-28 two-hubs-one-db corruption incident. Consequence:

- While an agent's stdio hub holds the db, `ctxone-hub --http --lens` on the same
  `--path` **refuses to start** (`database is already locked …`, exit 75).
- On reboot, whichever comes up first wins the lock. (Observed: Claude booted
  first → its stdio hub owns the db → no way to bring up Lens on that memory.)
- Two agents can't share one memory graph; each wants its own stdio child.

The `ctx` CLI and Lens already prove the HTTP path works for any client. The
missing piece is **MCP as an HTTP transport**, so agents connect to a shared hub
by URL instead of spawning their own.

## Feasibility (confirmed)

`rmcp` 1.7.0 (already a dependency) exposes the
`transport-streamable-http-server` feature (pulls in `server-side-http`). It
provides a `StreamableHttpService` that mounts as a `tower`/axum service — i.e.
it can be added as a route on the **existing** axum `Router`. We currently build
only `["server", "transport-io", "macros"]`.

## Target topology

```
  one launchd / brew-services daemon:
  ctxone-hub --http --lens --path ~/.ctxone/memory.db   (owns the db, one lock)
        │
        ├── GET  /                     → Lens web UI
        ├── /api/*                      → REST (CLI, integrations)
        └── /mcp   (streamable HTTP)    → MCP tools for every agent

  agents connect by URL, spawn nothing:
    Claude / Cursor / Codex  ──HTTP──▶  http://localhost:3001/mcp
```

## The one real wrinkle: identity comes from cwd today

The stdio path derives **namespace** and **git-branch mirror** from the *spawning
process's cwd* ([main.rs](../../server/src/main.rs) stdio branch): `.ctxproject`
walk-up → git-remote lookup → namespace; current git branch → mirrored memory
branch. A shared daemon has **no per-client cwd**.

Resolution — reuse the mechanism the HTTP API already has. Namespace is resolved
per-request by the `NamespaceId` extractor
([server/src/http.rs](../../server/src/http.rs)): `?namespace=` query → then
`X-CTXone-Namespace` header → then `default`. So:

- **Namespace** for an MCP client is set **statically in that agent's MCP config**,
  via a URL query (`/mcp?namespace=ctxone-myproj`) or a static header. One agent
  config = one project namespace. (A single desktop agent working across many
  repos would use `default` or per-workspace configs — documented as a known
  limitation of the daemon model.)
- **Branch mirroring from live git state** cannot happen daemon-side; default ref
  is `main` unless the client passes a branch. Agents/CLI can still target
  branches explicitly. (Auto-mirroring stays available in the stdio fallback.)

## Work breakdown

1. **Cargo** — add `transport-streamable-http-server` to the workspace `rmcp`
   features in [Cargo.toml](../../Cargo.toml) and `server/Cargo.toml`.
2. **Mount `/mcp`** on the router in
   [server/src/http.rs](../../server/src/http.rs) (`router_with_config_inner`).
   Build a `StreamableHttpService` whose per-session factory constructs a
   `CtxOneServer` scoped to the namespace resolved from the request
   (`?namespace=` / `X-CTXone-Namespace`), reusing `repo_for(ns)`. Verify how
   rmcp 1.7 surfaces request headers to the session factory; if it doesn't,
   scope via the URL query captured at the route layer (preferred anyway).
3. **main.rs** — in `http_mode`, always (or behind `--mcp-http`, default on)
   attach the `/mcp` route. Keep the `else` stdio branch **as a documented
   legacy/offline fallback**. The lockfile stays — it now guards the single
   daemon, which is exactly what we want.
4. **`ctx init` / `mcp_server_entry`** ([cli/src/main.rs](../../cli/src/main.rs))
   — teach it to emit a **URL transport** entry instead of the stdio spawn:
   - Add `--transport http|stdio` (default `http`).
   - HTTP entry per client dialect: Claude/Cursor `{"url":"http://localhost:3001/mcp?namespace=…"}` (or `"type":"http"`),
     Codex TOML equivalent. Keep the stdio `{"command","args"}` entry available
     via `--transport stdio`.
   - Preserve `--agent-id` as `X-CTXone-Agent` (header or query) so `ctx blame`
     still attributes commits per tool.
5. **Service install** — document (and optionally generate) a launchd plist /
   `brew services` unit that runs `ctxone-hub --http --lens` at login, so the
   daemon is up independent of any agent. See [../DEPLOYMENT.md](../DEPLOYMENT.md).
6. **Docs** — [../DEPLOYMENT.md](../DEPLOYMENT.md) (done), ARCHITECTURE topology
   correction (done), and update QUICKSTART/INTEGRATIONS once code lands.

## Migration / compatibility

- Existing stdio configs keep working (fallback path untouched). `ctx init
  --transport stdio` regenerates the old form.
- First `ctx init` after this ships rewrites detected agents to the URL form and
  prints a one-line note that a running daemon is now required (with the service
  command).
- No db format change; same graph, same file. The daemon simply becomes the sole
  writer instead of a per-agent child.

## Risks / open questions

- **rmcp header access per MCP session** — needs a spike; URL-query scoping is
  the fallback and is arguably cleaner (static per agent config).
- **Multi-repo desktop agent** — one static namespace per config is a real
  limitation vs. cwd auto-detection. Acceptable for the daemon model; the stdio
  fallback remains for users who want per-cwd auto-scoping.
- **Auth** — `/mcp` on localhost inherits the existing per-IP rate limit; no auth
  today. Fine for loopback; note it before any non-local bind.
- **Lifecycle** — daemon must be supervised (launchd `KeepAlive`) so a crash
  doesn't silently drop MCP for all agents.

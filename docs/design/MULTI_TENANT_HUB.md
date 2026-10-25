# Design: One Hub, many tenants

**Status:** In progress. AgentStateGraph's shared pool (t-001) and tenant
identity (t-002) are in; per-tenant state (t-003) is next. Until it lands,
`--tenants-file` checks the file and exits.
**Plan:** CTX plan `ctx-multi-tenant-hub` (tasks listed at the end).
**Goal:** One `ctxone-hub` process serves many users ("tenants"), each with
their own memory, plans, sessions and project registry, on one Postgres
database. A shared deployment becomes one Hub and one database instead of a Hub
process, a route and a connection pool per user.

## Today: one tenant per process

A Hub serves exactly one tenant, fixed at startup:

- `--tenant` picks the tenant when the Postgres storage connects. `HubState`
  holds one `repo`, one `SessionRegistry` and one `HubStore`
  ([server/src/http.rs](../../server/src/http.rs)).
- Tokens (`--auth-token`, `--auth-token-file`) are a flat set. Every accepted
  token reaches the whole graph; nothing maps a token to a person.
- 93 places in the REST handlers reach data through
  `HubState::repo_for(namespace)`. About 60 more use `s.repo`, `s.sessions` or
  `s.store` directly. MCP over HTTP caches one service per (workspace, session)
  on the same repository ([server/src/mcp_http.rs](../../server/src/mcp_http.rs)).
- Process-wide by design: session statistics, merge guards, the rate limiter
  (keyed by peer IP), the ASD process pool, the transcript sweep and autosync,
  SQLite backups and the lockfile, and startup schema migrations.
- Lens sends no credentials. It relies on the loopback exemption and
  same-origin checks.

A shared deployment therefore needs a Hub process per user, a hostname or route
per user to reach it, and a Postgres pool per process: AgentStateGraph's default
is up to 32 connections, which a small database runs out of after a few users.

## Target: the tenant comes with the request

```
  Alice's agents ─ token A ─┐
  Bob's agents   ─ token B ─┤  HTTPS, one hostname
  Lens           ─ token  ──┘
                            │
  ctxone-hub --http --tenants-file /etc/ctxone/tenants
        ├── auth: token → tenant
        ├── tenant cache:  alice → { repo, hub tables, session stats, MCP services }
        │                  bob   → { … }
        └── one Postgres pool ──▶ Postgres (every row tagged with its tenant)
```

### Principles

1. **A request belongs to exactly one tenant,** decided once, by the auth layer,
   from its credential. Nothing downstream chooses a tenant.
2. **No tenant-less path to data.** In multi-tenant mode `HubState` holds no
   repository, registry or store; a handler gets them only from the request's
   tenant, so code that forgets the tenant doesn't compile.
3. **Single-tenant mode is unchanged and stays the default:** `ctx serve`,
   SQLite, `--tenant`. It behaves as one implicit tenant.
4. **Workspaces (namespaces) stay as they are, inside a tenant.** Two tenants can
   both have a `ctxone` workspace, a project `app` and a session `s1`.

### Identity: token to tenant

- `--tenants-file <PATH>` turns multi-tenant mode on. Each line is
  `<tenant> <sha256 of a token>`; a tenant may have several lines (one per
  device). Only hashes are stored: a token is shown once, when it's created.
  The file is re-read every second, like `--auth-token-file`, so adding,
  rotating or removing a tenant needs no restart.
- The auth middleware hashes the bearer token, looks it up, and attaches the
  `Tenant` to the request; an unknown token is a 401. There's no loopback
  exemption in this mode, because there is no "local user". Hashes are compared
  in constant time.
- A `TenantResolver` trait is the seam. The file resolver ships with the Hub; an
  account service (sign-in, per-device tokens, suspension) can supply its own.
- Rate limits are per tenant, not per peer IP.
- Tenant names are `[a-z0-9-]{1,63}`. A tenant name is the AgentStateGraph
  `tenant_id` and the `tenant_id` of the `ctxone_*` tables
  ([server/src/hub_store.rs](../../server/src/hub_store.rs)).

### Per-tenant state

`HubState` gets a tenant cache: tenant → `Arc<TenantState>`, holding:

- **the repository:** AgentStateGraph over the shared pool (below);
- **the hub tables:** a `PgStore` for the project registry and session statistics,
  which are already tenant-scoped;
- **session statistics:** the tenant's own `SessionRegistry`, loaded from
  `ctxone_sessions` when the tenant opens;
- **MCP services:** one per (workspace, session), as today.

A tenant opens on its first request. Opening runs the AgentStateGraph and CTXone
schema migrations for that tenant under a per-tenant advisory lock, so two Hubs
opening the same tenant can't race, then loads its statistics. A tenant idle for
a while (30 minutes, say) is evicted after its statistics are flushed.

Still process-wide: the connection pool, the cap on concurrent project
detections, and the rate limiter (now keyed by tenant). Merge guards are keyed
by (tenant, workspace, target).

### AgentStateGraph: tenants share one pool

`PostgresStorage::for_tenant(&self, tenant)` returns a storage on the same pool
with another `tenant_id` and its own garbage-collection state. Schema setup runs
once per process, not per tenant. This is the Postgres part of AgentStateGraph's
`StoreProvider` (its DESIGN.md, Plan F t-003); Postgres row-level security can
follow as a backstop. The pool size becomes a Hub setting (`--pg-pool-size`)
because every tenant now shares it.

### What multi-tenant mode turns off

These act on the Hub machine's own disk, which means nothing to a remote user:

- the ASD code-intelligence process pool and proxy (`--asd-path`, `repos.toml`
  discovery). A tenant's code intelligence will come from its synced ASD stores
  later;
- the transcript sweep and autosync, which run `ctx ingest-session --all` over
  the Hub machine's transcripts;
- SQLite backups and the lockfile (Postgres has its own backups);
- project detection's filesystem fallback. `ctx` sends what it found in the
  directory (`client_read=true`, [HTTP_API.md](../HTTP_API.md)).

All of them stay as they are in single-tenant mode.

### Lens

Lens asks for the tenant's token once and sends it as a bearer token on every
call, kept in the browser's storage for the Hub's origin. `--strict-origin` with
the Hub's own origin keeps other sites out. A real sign-in comes with an account
service.

### Background work

The statistics flush loop walks the open tenants every 30 seconds. Evicting a
tenant and shutting down flush it too.

### Administration

`ctxone-hub tenant` edits the tenants file and the database:

| Command | Effect |
|---|---|
| `add <name>` | Create the tenant; print a new token once |
| `list` | Tenants and their token counts |
| `rotate <name>` | Replace a tenant's tokens; print the new one |
| `disable <name>` / `enable <name>` | Refuse or allow its tokens; data kept |
| `purge <name>` | Delete every row for the tenant, in AgentStateGraph's tables and the `ctxone_*` tables. Asks for confirmation |

`export <name>` follows with AgentStateGraph's store export.

## Isolation

The main risk is a request reaching another tenant's data. The defences:

1. **Structure.** The tenant is decided once. In multi-tenant mode `HubState`
   has no data handle; handlers take them from a `Tenant` extractor.
2. **Tests.** For every REST endpoint and MCP tool, tenant B tries to read, list,
   search and write tenant A's data, using the same workspace names, project ids
   and session ids, and must see nothing.
3. **Storage.** AgentStateGraph filters every query by `tenant_id` today;
   Postgres row-level security follows.
4. **Logs** carry the tenant name and never a token.

## Deployment shape

- One Hub (a single replica to start), one Postgres, one hostname. Per-user
  subdomains become optional.
- Adding a user is `ctxone-hub tenant add alice`.
- One shared connection pool.
- More than one replica later needs MCP session affinity (a streamable-HTTP MCP
  session lives in one process) and additive statistics flushes (today a flush
  overwrites the tenant's row).

## Not in scope

- Accounts, sign-in and billing. An account service plugs in through
  `TenantResolver`.
- Several people in one tenant with separate attribution. One tenant with
  several tokens works; per-token attribution comes later.
- Sharing between tenants.
- Per-tenant access to synced ASD stores.

## Plan

CTX plan `ctx-multi-tenant-hub`:

| Task | What | Needs |
|---|---|---|
| t-001 | AgentStateGraph: tenants share one Postgres pool (`for_tenant`); release | |
| t-002 | Tenant identity: `--tenants-file`, token → tenant, `TenantResolver`, per-tenant rate limits | |
| t-003 | Per-tenant state: tenant cache, lazy open with per-tenant migrations, idle eviction; no tenant-less data path | t-001, t-002 |
| t-004 | Background work per tenant; turn off machine-local features in multi-tenant mode | t-003 |
| t-005 | Isolation tests across every endpoint and MCP tool | t-003 |
| t-006 | `ctxone-hub tenant` administration, including purge | t-002, t-003 |
| t-007 | Lens on a multi-tenant Hub (token entry, bearer calls) | t-003 |
| t-008 | Docs: DEPLOYMENT section, `--pg-pool-size`, CHANGELOG | t-004, t-006, t-007 |
| t-009 | Later: Postgres row-level security; more than one replica | t-005 |

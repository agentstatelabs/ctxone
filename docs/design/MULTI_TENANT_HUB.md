# Design: One Hub, many tenants

**Status:** Implemented. `ctxone-hub --http --tenants-file` serves tenants
their data and Lens, and `ctxone-hub tenant` manages them. That took:
AgentStateGraph's shared pool (t-001), tenant identity (t-002), per-tenant
state (t-003), background work with the machine-local features off (t-004),
isolation tests across every route and MCP tool (t-005), tenant administration
(t-006), Lens (t-007) and the deployment docs (t-008). How to run one:
[DEPLOYMENT.md](../DEPLOYMENT.md#one-hub-for-many-users). Postgres row-level
security, a backstop behind every query's tenant filter, followed (t-009:
AgentStateGraph's policies in t-010, the Hub's tables and
`ctxone-hub db row-security` in t-011), and then more than one Hub on the
database (t-012): MCP sessions any of them can continue, and statistics
flushes that add up ([More than one replica](#more-than-one-replica)).
**Plan:** CTX plan `ctx-multi-tenant-hub` (tasks listed at the end).
**Goal:** One `ctxone-hub` process serves many users ("tenants"), each with
their own memory, plans, sessions and project registry, on one Postgres
database. A shared deployment becomes one Hub and one database instead of a Hub
process, a route and a connection pool per user.

## Before: one tenant per process

A Hub served exactly one tenant, fixed at startup, as a single-tenant Hub still
does:

- `--tenant` picked the tenant when the Postgres storage connected. `HubState`
  held one `repo`, one `SessionRegistry` and one `HubStore`
  ([server/src/http.rs](../../server/src/http.rs)).
- Tokens (`--auth-token`, `--auth-token-file`) were a flat set. Every accepted
  token reached the whole graph; nothing mapped a token to a person.
- 93 places in the REST handlers reached data through
  `HubState::repo_for(namespace)`. About 60 more used `s.repo`, `s.sessions` or
  `s.store` directly. MCP over HTTP cached one service per (workspace, session)
  on the same repository ([server/src/mcp_http.rs](../../server/src/mcp_http.rs)).
- Process-wide by design: session statistics, merge guards, the rate limiter
  (keyed by peer IP), the ASD process pool, the transcript sweep and autosync,
  SQLite backups and the lockfile, and startup schema migrations.
- Lens sent no credentials. It relied on the loopback exemption and
  same-origin checks.

A shared deployment therefore needed a Hub process per user, a hostname or route
per user to reach it, and a Postgres pool per process: AgentStateGraph's default
is up to 32 connections, which a small database runs out of after a few users.

## Design: the tenant comes with the request

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
- As built: `GET /api/health` needs no token, so load balancers and container
  health checks can probe the Hub. Its answer is fixed, so it opens no tenant,
  and it skips the rate limiter. A token sent with it is still checked.
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

A tenant opens on its first request that needs its data. Opening runs the
AgentStateGraph and CTXone schema migrations for that tenant under a per-tenant
advisory lock, so two Hubs opening the same tenant can't race, then loads its
statistics. A tenant idle for a while (30 minutes, say) is evicted after its
statistics are flushed.

As built (`server/src/tenant_cache.rs`):

- The lock is `pg_advisory_lock(OPEN_LOCK_CLASS, hashtext(<tenant>))`, taken on
  a connection detached from the pool. Closing that connection releases the lock
  however the open ends, and the pool keeps its full size for the migrations.
- A Hub opens one tenant at a time, so requests arriving together for a new
  tenant open it once.
- Eviction skips a tenant a request is holding, and one a request reached while
  its statistics were flushing.
- The Hub Home's workspace summary, reused for 15 seconds, is kept per tenant.

Still process-wide: the connection pool, the cap on concurrent project
detections, and the rate limiter (now keyed by tenant). Merge guards are keyed
by (tenant, workspace, target).

### AgentStateGraph: tenants share one pool

`PostgresStorage::for_tenant(&self, tenant)` returns a storage on the same pool
with another `tenant_id` and its own garbage-collection state. Schema setup runs
once per process, not per tenant. This is the Postgres part of AgentStateGraph's
`StoreProvider` (its DESIGN.md, Plan F t-003); Postgres row-level security
backs it up (AgentStateGraph 1.5.0). The pool size becomes a Hub setting
(`--pg-pool-size`) because every tenant now shares it.

### What multi-tenant mode turns off

These act on the Hub machine's own disk, which means nothing to a remote user:

- the ASD code-intelligence process pool and proxy (`--asd-path`, `repos.toml`
  discovery). A tenant's code intelligence will come from its synced ASD stores
  later;
- the transcript sweep and autosync, which run `ctx ingest-session --all` over
  the Hub machine's transcripts;
- SQLite backups and the lockfile (Postgres has its own backups);
- project detection's filesystem fallback. `ctx` sends what it found in the
  directory (`client_read=true`, [HTTP_API.md](../HTTP_API.md));
- help from other tools: `help` reads the machine's help index and runs the
  binaries it names;
- the MCP code tools that run the `asd` CLI (`code_impact`,
  `code_cross_repo_edges`).

All of them stay as they are in single-tenant mode.

As built: the HTTP routes for these answer 404 with the reason
(`http::MACHINE_LOCAL_ROUTES`), detection without `client_read` is a 400, and
the MCP code tools say there's nothing to reach. A multi-tenant router ignores
the configuration for them, and the binary refuses their flags at startup.

### Lens

Lens asks for the tenant's token once and sends it as a bearer token on every
call, kept in the browser's storage for the Hub's origin. `--strict-origin` with
the Hub's own origin keeps other sites out. A real sign-in comes with an account
service.

As built: Lens's own files need no token; every call it makes to the API, the
Code views' included, sends the token it keeps in `localStorage`. A 401 opens
token entry, which checks a token with `GET /api/tenant` before keeping it. A
refused token is forgotten, so a rotated or disabled one brings token entry
back. The sidebar names the tenant and can forget the token. With `--lens` and
`--strict-origin`, the Hub won't start unless `--allowed-origin` lists an
origin, which should be its own. The same token entry serves a single-tenant
Hub with a token, reached from another machine, which needed an SSH tunnel
before; a local Hub never asks.

Lens asks `GET /api/tenant` on every load whether the Hub names a tenant, which
only a multi-tenant Hub does, and then leaves out the views for what's off
there: Code, session import, autosync and transcript sync (`MachineLocal.svelte`
wraps them). A link to a Code view says there's no code intelligence on this
Hub.

### Background work

The statistics flush loop walks the open tenants every 30 seconds. Evicting a
tenant and shutting down flush it too.

As built: every 30 seconds, `TenantCache::spawn_upkeep` closes the tenants
none of whose tokens is accepted any more and those idle for 30 minutes, then
flushes the rest. The binary flushes every open tenant after the server stops.

The main graph's size refresher publishes one number for the whole process, which
fits a single-tenant Hub. A multi-tenant Hub runs none, so each session computes
its own tenant's, throttled.

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

As built (`server/src/tenant_admin.rs`):

- A disabled token is a line `disabled <tenant> <sha256 of a token>`: kept, and
  refused. The duplicate-hash check counts it, so a hash listed for another
  tenant is refused for both.
- `add` and `rotate` print the token alone on stdout, `ctx_` and 64 hex digits
  from the system's random source, and everything else on stderr. `add`
  refuses a tenant the file has; `rotate` refuses a disabled one.
- Edits keep comments and other lines, wait for each other on `<file>.lock`,
  and replace the file with one rename (through a symlink, to its target). A
  new file is `0600`; an existing one keeps its permissions. A file with a line
  a Hub can't use is left alone until it's fixed, and only `add` makes a
  missing file.
- A Hub closes a tenant once no token of its is accepted, at the next upkeep
  pass and after flushing it (`TenantResolver::admits`,
  `TenantCache::close_refused`). Purge depends on this: a flush writes every
  session's statistics, so a Hub still holding a purged tenant open would
  write its `ctxone_sessions` rows back, and keep serving its memory if the
  name were added again.
- `purge` takes only a tenant the file lists with every token disabled, so a
  mistyped `--tenants-file` can't delete a tenant in use, and asks for the
  tenant's name unless given `--yes`. Then it waits 61 seconds (two upkeep passes and a
  re-read of the file), so every Hub has closed the tenant, and checks again
  that no token was enabled meanwhile. It deletes in one transaction, holding
  the tenant's open lock, from a fixed list of tables: AgentStateGraph's, then
  `ctxone_project_paths` before the `ctxone_projects` they refer to, then
  `ctxone_sessions`. A table with a `tenant_id` column that isn't on the list
  stops it before anything is deleted, so a schema the Hub doesn't know can't
  keep a tenant's rows, and a test checks the list against the schema. The
  tenant's lines leave the file last.

## Isolation

The main risk is a request reaching another tenant's data. The defences:

1. **Structure.** The tenant is decided once. In multi-tenant mode `HubState`
   has no data handle; handlers take them from a `Tenant` extractor.
2. **Tests.** For every REST endpoint and MCP tool, tenant B tries to read, list,
   search and write tenant A's data, using the same workspace names, project ids
   and session ids, and must see nothing.

   As built (`server/tests/tenant_isolation.rs`, on Postgres): both tenants
   hold data under the same workspace, project id and remote, session ids,
   plans, branch, document and memory contexts, each text carrying its tenant's
   marker, plus one session of its own whose id carries the marker (with only
   shared ids, another tenant's session would hide behind one's own). B's probes use those names and A's own ids (fact paths, commit ids,
   reminder, taint and draft ids, A's MCP session). No answer may carry A's
   marker or ids; where B has data of its own the answer must carry B's marker;
   none of B's requests may open A; and A's reads before and after must match.
   The test reads the routes `build_router` mounts, and the tools MCP lists,
   so a route or tool without a probe fails it. Mutations that share storage,
   drop a `tenant_id` filter, share MCP services or share the recall cache are
   each caught.
3. **Storage.** AgentStateGraph filters every query by `tenant_id`, as the
   Hub does for its `ctxone_*` tables. With Postgres row-level security
   switched on (`ctxone-hub db row-security on`), the database checks it too:
   each of those tables has a policy comparing a row's `tenant_id` with the
   connection's tenant, which every connection the Hub uses sets first. The
   isolation sweep and the purge test run with it on as well, as a role it
   binds.
4. **Logs** carry the tenant name and never a token.

What tenants still share holds none of their data:

- **The recall cache** was one for the process, keyed by graph state without a
  tenant. A result could only be served to the tenant whose graph made it,
  because the key's commit id is a hash of that graph, but nothing else kept it
  there. Each tenant has its own now (`TenantState::recall_cache`).
- **The "no namespace" warning** was deduplicated in a set of request paths,
  which carry names clients choose: any client could grow it without bound, and
  one tenant's requests muted the warning for others. It keeps routes now.
- **Capacity:** the connection pool, the blocking thread pool, the limit of 16
  project detections running at once, and the lock under which one tenant
  opens at a time. A busy tenant can slow the others; per-tenant limits are
  later work. Each tenant keeps an MCP service per (workspace, session) with no
  cap. Closing an idle tenant ends its MCP services; on Postgres their sessions
  continue when it opens again.

## More than one replica

Hubs on one database share it and nothing else (t-012). What a process kept to
itself had to move to the database, or be safe to keep in each process:

- **MCP sessions.** A streamable-HTTP MCP session lives in the process that
  began it. rmcp (1.7) can continue one elsewhere from a `SessionStore` holding
  the client's `initialize` parameters: a process asked about a session it
  doesn't hold loads them and replays the handshake. `McpSessionStore`
  ([server/src/mcp_sessions.rs](../../server/src/mcp_sessions.rs)) keeps them in
  `ctxone_mcp_sessions`, by tenant and session id, with the workspace and
  statistics session the session serves, and continues it only for those, so a
  session id can't take a client into another tenant or workspace. That makes
  affinity on `Mcp-Session-Id` unnecessary, and covers what affinity couldn't:
  a session its own process let go (rmcp's five-minute idle timeout, a closed
  tenant, a restart). A row unused for 7 days is deleted when the tenant's next
  session is stored.
- **Session statistics.** A flush wrote each session's totals, so two processes
  overwrote each other's counts. Each `SessionStats` now remembers what its row
  held when this process last read or wrote it, and a flush adds only what's
  new. Under `pg_advisory_xact_lock(STATS_LOCK_CLASS, hashtext(<tenant>))` it
  reads the rows of the sessions it holds and of those written in the last
  minute, writes each row plus what's new here, and takes in what the others
  added, sessions they began included (`SessionRegistry::flush_to_pg`). A
  per-model split the backfill replaced is written whole. A session whose row
  is gone was deleted elsewhere, and is dropped rather than written back;
  deleting one takes the same lock. A SQLite hub is one process and still
  writes totals.
- **Kept in each process, and safe there:** the recall cache, keyed by the head
  commit, so another process's write invalidates it; the workspace summary,
  reused for 15 seconds; merge guards, behind which AgentStateGraph's ref
  compare-and-swap is what keeps a commit from being lost; and the rate
  limiter, which is per replica.

## Deployment shape

[DEPLOYMENT.md](../DEPLOYMENT.md#one-hub-for-many-users) is the operator's
guide.

- One Hub (or several replicas), one Postgres, one hostname. Per-user
  subdomains become optional.
- Adding a user is `ctxone-hub tenant add alice`.
- One shared connection pool.
- Replicas need no affinity at the load balancer
  ([More than one replica](#more-than-one-replica)).

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
| t-010 | AgentStateGraph: row-level security policies, off until switched on; a tenant-scoped pool; release 1.5.0 | |
| t-011 | Row-level security on the Hub's tables; `ctxone-hub db row-security` | t-010 |
| t-012 | More than one Hub replica: MCP sessions any replica continues, additive statistics flushes | |

Every task is done; t-010 to t-012 were t-009's parts.

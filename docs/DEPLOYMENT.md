# Deployment & Configuration

How to run the CTXone Hub, and the configuration choices that matter — the
**shared daemon** (the standard setup), running it **as a service**, giving an
agent its **own memory**, **securing** the hub with a token when you expose
it beyond localhost, and **one hub for many users**
([below](#one-hub-for-many-users)). If you followed the Quickstart and
everything works, you already have the standard setup; this page is the
reference for the rest.

## The one rule that governs every setup

**One hub owns one db file at a time.** On startup a hub writes a PID lockfile
next to the db (`<db>.lock`). If a second hub tries to open the same `--path`
while the first is alive, it **refuses to start**:

```
database is already locked by ctxone-hub pid 12345 (lockfile: …/memory.db.lock);
refusing to start a second hub against the same db
```

This is deliberate — two hubs writing one SQLite file corrupts it (the
2026-04-28 incident). See [DATA_SAFETY.md](DATA_SAFETY.md). Everything below is
really about *who* holds that single lock.

## How a hub can run

`ctxone-hub` has two transports for talking to agents, plus the HTTP surfaces:

| Mode | Command | Serves | Owns the db |
|------|---------|--------|-------------|
| **MCP stdio** | `ctxone-hub --path DB` | MCP tools to the spawning agent | yes, while the agent runs |
| **HTTP (+Lens)** | `ctxone-hub --http --lens --path DB` | REST API + web UI + **MCP at `/mcp`** | yes, while the daemon runs |

**The standard setup is one shared HTTP daemon.** An `--http` hub serves MCP (at
`/mcp`, Streamable HTTP) + REST + the Lens web UI from one process, and
`ctx init` (default `--transport http`) points every tool at it by URL — no
lockfile race, web UI always up, all tools share one graph. `ctx init
--transport stdio` is the escape hatch: each tool spawns its own hub that holds
the lock, so you can't also run a web UI on that db. Design notes:
[design/UNIFIED_HUB.md](design/UNIFIED_HUB.md).

## Choosing a topology

Most setups want **Topology B (the shared daemon)** — it's what `ctx init` writes
by default. Topology A is the no-daemon fallback; C and D cover isolation and
one-off inspection.

### A. Agent-only stdio (no daemon, fallback)

`ctx init --transport stdio` — each tool spawns and owns its own stdio hub.
Nothing else touches the db.

- ✅ Zero extra processes, no port, no daemon to run.
- ❌ No Lens; can't attach the CLI-over-HTTP to *this* db while the agent runs
  (the CLI reads the same graph, but only through a running HTTP hub). One tool
  per db — a second tool needs its own db (topology C).

### B. Shared HTTP daemon — the default & standard

Run **one** long-lived hub that serves everything, and point agents, the CLI, and
Lens at it. Nothing spawns its own child; there is exactly one db owner. This is
what a plain `ctx init` configures.

```bash
# 1. Run the one daemon (ideally as a login service — see below — so it's up at boot).
ctxone-hub --http --lens --path ~/.ctxone/memory.db

# 2. Point the CLI at it (defaults to http://localhost:3001, so usually optional).
export CTX_SERVER=http://localhost:3001

# 3. Configure your tools to connect by URL. http is the default, so plain:
ctx init                       # all detected tools
ctx init --tool claude         # or one tool
```

`ctx init` (default `--transport http`) writes an MCP entry like:

```jsonc
{
  "mcpServers": {
    "ctxone": { "type": "http", "url": "http://localhost:3001/mcp?namespace=<detected>" }
  }
}
```

The `?namespace=` is filled in from the project detected in the current directory
(same scoping a per-project stdio hub would use). Override the endpoint with
`--mcp-url`, e.g. `--mcp-url http://mybox:3001/mcp`.

- ✅ One process, one lock, **startup order irrelevant** — the reboot race is gone.
  Lens always up, independent of any agent; multiple agents share one graph.
- MCP clients reach the daemon two ways, and `ctx init --transport http` writes
  the right one per client:
  - **Native URL** — Claude Code, Cursor, VS Code get `{"type":"http","url":…}`.
  - **`mcp-remote` bridge** — **Claude Desktop** has no native HTTP MCP support,
    so `ctx init` writes an stdio bridge
    (`npx -y mcp-remote <url> --transport http-only`) that proxies to the daemon.
    Requires Node/npx on PATH; a note is printed when this is used.
  - **Codex** — native HTTP via a `url` key in `config.toml`
    (`[mcp_servers.ctxone]\nurl = "…/mcp?namespace=…"`).
- ⚠️ A single daemon has no per-cwd project detection, so **one agent config maps
  to one namespace** (baked into the URL). An agent that roams many repos uses one
  fixed namespace or per-workspace configs. Need per-cwd auto-scoping? Use stdio
  (topology A) — it's still fully supported.

### C. Give an agent its own dedicated db (isolation)

Point one agent at a **different `--path`** so it never contends with your shared
hub or another agent. Two separate files → two separate locks → no conflict, and
two independent memory graphs.

Edit that agent's MCP config (or run `ctx init --tool <name>` after setting the
path). The stdio entry looks like:

```jsonc
// e.g. ~/Library/Application Support/Claude/claude_desktop_config.json
{
  "mcpServers": {
    "ctxone": {
      "command": "/opt/homebrew/bin/ctxone-hub",
      "args": ["--path", "/Users/user/.ctxone/claude.db", "--agent-id", "claude-desktop"]
    }
  }
}
```

Use this when:

- You want Claude's memory kept **separate** from a shared/team graph.
- Two agents on one machine should **not** see each other's memory.
- You're experimenting and don't want to touch your main graph.

> Isolated dbs do **not** sync. A fact remembered in `claude.db` is invisible to
> the hub serving `memory.db`. To browse an isolated db in Lens, run a hub on it
> at a **different port** while the agent is stopped, or on a copy:
> `ctxone-hub --http --lens --path /Users/user/.ctxone/claude.db --port 3002`.

### D. Browse a live agent's memory without fighting the lock

You can't open a second hub on a locked db. To look at the graph an agent is
actively using, snapshot it and serve the copy:

```bash
cp ~/.ctxone/memory.db /tmp/lens.db
ctxone-hub --http --lens --path /tmp/lens.db --port 3002
```

Read-only snapshot; it won't reflect writes after the copy. (The proper fix is
the shared daemon in topology B — one owner, Lens live on the same graph the
agents write.)

## Configuration reference

### Where the db lives

`ctx init` points agents at the canonical path:

- macOS / Linux: `~/.ctxone/memory.db`
- Windows: `%APPDATA%\ctxone\memory.db`

Override per agent with `--path` (topology C). A bare `ctxone-hub` with no
`--path` defaults to `./target/ctxone.db` (dev convenience) — always pass
`--path` in real setups.

### Useful flags

| Flag | Purpose |
|------|---------|
| `--path <PATH>` | SQLite db file (per-agent isolation lives here) |
| `--http` | Enable the REST API |
| `--lens` | Serve the web UI at `/` (requires `--http`) |
| `--port <N>` | HTTP port (default 3001) — use distinct ports for parallel dbs |
| `--agent-id <NAME>` | Attribution recorded on commits (`ctx blame`) |
| `--auth-token <TOK>` | Bearer token guarding REST + `/mcp` (env `CTXONE_AUTH_TOKEN`); loopback exempt |
| `--allowed-origin <ORIGIN>` | Extra browser origin allowed to call the API (repeatable; same-origin is always allowed) |
| `--init` | Create the db file if missing (guards against path typos) |
| `--storage memory` | Ephemeral, no file, no lock — good for tests/demos |
| `--storage postgres --database-url <URL>` | Postgres instead of SQLite (or set `DATABASE_URL`) |
| `--pg-pool-size <N>` | Most Postgres connections the hub holds (default 32); a multi-tenant hub's tenants share them |
| `--tenants-file <PATH>` | One hub for many users: each request's token picks its tenant ([below](#one-hub-for-many-users)) |
| `--asd-path name=PATH` | Register an ASD code-graph repo for the process pool |

### Environment

| Var | Effect |
|-----|--------|
| `CTX_SERVER` | Hub URL the `ctx` CLI targets (default `http://localhost:3001`) |
| `CTX_NAMESPACE` | Project namespace for CLI/MCP scoping |
| `CTX_BRANCH` | Default memory branch/ref |
| `CTX_TOKEN` | Bearer token the `ctx` CLI sends (for a remote authed hub) |
| `CTX_REMOTE_TURNS` | `1` lets `ctx` send raw session turns to a hub that isn't on this machine (overrides the `remote_turns` config key; see below) |
| `CTXONE_AUTH_TOKEN` | Same as `--auth-token` (bearer guarding REST + `/mcp`) |
| `CTXONE_AUTH_TOKEN_FILE` | Same as `--auth-token-file` (tokens one per line, re-read every second) |
| `CTXONE_REQUIRE_AUTH_ON_LOOPBACK` | `1` = same as `--require-auth-on-loopback` (loopback peers need the token too) |
| `CTXONE_ALLOWED_ORIGINS` | Comma/space-separated extra allowed browser origins |
| `CTXONE_STRICT_ORIGIN` | `1` = same as `--strict-origin` (only the allowed origins, never same-origin by `Host`) |
| `DATABASE_URL` | Postgres connection URL; setting it selects Postgres storage, unless `--storage memory` |
| `CTXONE_PG_POOL_SIZE` | Same as `--pg-pool-size` |
| `CTXONE_TENANTS_FILE` | Same as `--tenants-file`, for the hub and for `ctxone-hub tenant` |
| `CTXONE_BACKUP_KEEP` | Startup snapshots to retain (default 5) |
| `CTXONE_BACKUP_INTERVAL_SECS` | Background VACUUM-INTO snapshot interval (default 1800; 0 disables) |
| `RUST_LOG` | Log level (`info` default) |

## Authentication

The hub binds `0.0.0.0` (all interfaces). With no token set, REST and `/mcp` are
reachable from the network **unauthenticated** — the hub logs a loud warning at
startup. Before exposing it beyond localhost, set a bearer token:

```bash
ctxone-hub --http --lens --path ~/.ctxone/memory.db --auth-token "$(openssl rand -hex 32)"
# or: CTXONE_AUTH_TOKEN=… ctxone-hub --http …
```

Behaviour:
- **Loopback peers (127.0.0.1/::1) are exempt** — local CLI, Lens, and
  same-host agents keep working with no token — unless
  `--require-auth-on-loopback` is set (see *Hosted hub* below).
- **Non-loopback requests must send** `Authorization: Bearer <token>` (else 401),
  except `GET /api/health`, so health checks need no token.
- Setting a token also relaxes `/mcp`'s loopback-only Host check so authenticated
  remote clients can connect (the bearer becomes the gate).

How each client gets the token:
- **`ctx` CLI** — pass `--token <TOK>` or set `CTX_TOKEN` (only for a remote hub).

Raw session turns stay on the machine that recorded them. When `ctx`'s hub isn't
on this machine (anything but `localhost`, a `.localhost` name, 127.x or `::1`),
`ctx ingest-session` and `ctx session import` send each session's title,
metadata, provenance and extracted memories, and the Stop-hook
`ctx capture-turn` sends token counts and extracted memories, but none of them
sends the turns. An import's per-model token usage is derived from the turns,
so it doesn't show on that hub. To send turns as well, run
`ctx config set remote_turns true`, or set `CTX_REMOTE_TURNS=1` for one command.
- **AI tools** — `ctx init --transport http` writes it into each tool's config:
- **`ctx init --transport http --auth-token <TOK>`** — embeds a literal bearer:
  a `headers` entry for native http (Claude Code/Cursor/VS Code) and a
  `--header` arg for the mcp-remote bridge (Claude Desktop). The token is written
  in plaintext into those config files (a warning is printed).
- **`ctx init --transport http --auth-token-env <VAR>`** — keeps the secret out
  of config files: Codex gets `bearer_token_env_var = "<VAR>"`, and native http
  entries reference `Bearer ${VAR}` (for clients that expand env vars). The
  mcp-remote bridge can't expand env vars, so Claude Desktop needs the literal
  `--auth-token` form.

### Rotating tokens

Keep tokens in a file instead of on the command line, one per line:

```bash
ctxone-hub --http --auth-token-file /etc/ctxone/tokens
```

The hub re-reads the file every second and accepts every token in it, so a
rotation needs no restart: add the new token as a line, switch clients to it,
then delete the old line. An empty file accepts no token. A Kubernetes secret
mounted as a volume works as is, since its updates swap the file atomically; one
mounted with `subPath` never updates.

### Hosted hub

A hub reached over the internet, behind an ingress, should also stop trusting
loopback and `Host`:

```bash
ctxone-hub --http --storage postgres \
  --auth-token-file /run/secrets/ctxone-tokens \
  --require-auth-on-loopback \
  --strict-origin --allowed-origin https://alice.ctxone.example
```

- `--require-auth-on-loopback`: an ingress or sidecar on 127.0.0.1 makes every
  request arrive from loopback, which would otherwise skip the token. The hub
  refuses this flag without a token, and hands its own `ctx` subprocesses the
  token as `CTX_TOKEN`.
- `--strict-origin`: browsers may call the API only from the listed origins.
  Without it, an `Origin` matching the request's `Host` counts as same-origin,
  and behind an ingress, or under DNS rebinding, `Host` is whatever the caller
  sent.

A hub set up this way serves one tenant (`--tenant`): everyone with a token
shares one graph. For several users, each with their own memory, run one hub
with a tenants file instead: [One hub for many users](#one-hub-for-many-users).

### Postgres over TLS

The hub reaches Postgres over verified TLS, set in `DATABASE_URL` the way libpq
does it:

```bash
export DATABASE_URL='postgres://ctxone:secret@db.example.com:5432/ctxone?sslmode=verify-full'
```

- `sslmode=verify-full` (or `require`, or `verify-ca`): TLS, with the server's
  certificate checked against Mozilla's roots and its host name. There is no
  unverified TLS.
- A provider that signs with its own CA (Amazon RDS, Google Cloud SQL, …):
  download its CA bundle and add `&sslrootcert=/path/to/ca.pem`, or set
  `PGSSLROOTCERT=/path/to/ca.pem`.
- No `sslmode`, or `sslmode=disable`, is plaintext: fine for a database on the
  same machine, and on a private network if you mean it, in which case say
  `sslmode=disable`. For a database on another machine with no `sslmode`, or
  with `prefer`, the hub logs a warning at startup.

The startup line says which mode the hub connected with
(`Storage: postgres … tls=verify-full`); it never logs the URL, which may
carry a password. `scripts/test-postgres-tls.sh` runs the hub's TLS tests
against a throwaway Postgres that accepts only TLS.

### Cross-origin (CSRF / DNS-rebinding)

Independent of the token, the hub rejects **browser** requests whose `Origin`
isn't same-origin or allow-listed — so a page you visit can't drive the loopback
hub (loopback peers are auth-exempt, which is why this guard matters). CLI and
native MCP clients send no `Origin` and are unaffected; Lens is same-origin and
always allowed. CORS reflects only same-origin/allow-listed origins (never `*`).

To let a real cross-origin browser app (e.g. Lens hosted elsewhere) use the API:

```bash
ctxone-hub --http --allowed-origin https://lens.example.com   # repeatable
# or CTXONE_ALLOWED_ORIGINS="https://a.example, https://b.example"
```

### Lens over an authenticated hub

When a token is set, a **remote** browser's first call gets a 401 (loopback is
exempt, remote is not), and Lens asks for the token. Paste the hub's token: Lens
checks it, keeps it in the browser's storage for the hub's origin, and sends it
as `Authorization: Bearer` on every call. A refused token (a rotated one, say)
brings the question back, and *Forget token* in the sidebar removes it. Reach a
remote hub over HTTPS, so the token isn't sent in the clear. With
`--strict-origin`, list the hub's own origin with `--allowed-origin`.

To keep the hub off the network instead, reach it over an SSH tunnel, which
makes your browser a loopback client that needs no token:

```bash
ssh -L 3001:localhost:3001 you@hub-host      # then open http://localhost:3001
```

## One hub for many users

One hub can serve many people ("tenants") from one process and one Postgres
database. Each tenant has its own memory, plans, workspaces, project registry,
session statistics and MCP sessions, and two tenants can use the same workspace
names without meeting. Each request's token picks its tenant, and adding a
tenant is one command, with no restart.

```
  alice's agents, ctx, Lens ── token A ─┐
                                        ├─ HTTPS ─▶ proxy ─▶ ctxone-hub --tenants-file ─▶ Postgres
  bob's agents, ctx, Lens   ── token B ─┘
```

How it's built and how tenants are kept apart:
[design/MULTI_TENANT_HUB.md](design/MULTI_TENANT_HUB.md). The API's side of it:
[HTTP_API.md](HTTP_API.md#multi-tenant-mode).

### What it takes

- **Postgres.** One database holds every tenant. The hub creates its tables, and
  a tenant's rows on that tenant's first request. Reach the database over TLS
  ([Postgres over TLS](#postgres-over-tls)). To try multi-tenant mode out
  without one, `--storage memory` keeps the tenants in memory until the hub
  stops.
- **One hostname with HTTPS** in front of the hub, from a reverse proxy or an
  ingress ([below](#https-in-front)). The hub itself speaks plain HTTP.
- **One hub process.** A second hub on the same database would split a tenant's
  MCP sessions between processes, and each would overwrite the other's session
  statistics for the tenant.

### Add tenants

```bash
ctxone-hub tenant add alice --tenants-file /var/lib/ctxone/tenants
```

This prints alice's token on stdout, once: `ctx_` and 64 hex digits. Give it to
alice. The tenants file keeps only its SHA-256, a line `alice <hash>` per token.
Set `CTXONE_TENANTS_FILE` instead of passing `--tenants-file` every time.

| Command | Effect |
|---|---|
| `add <name>` | Create a tenant and print its token, once |
| `list` | The tenants and how many tokens each has |
| `rotate <name>` | Replace the tenant's tokens with a new one, printed once. The old ones are refused within a second |
| `disable <name>` | Refuse the tenant's tokens. Its data stays, and the hub closes the tenant within a minute |
| `enable <name>` | Accept a disabled tenant's tokens again |
| `purge <name>` | Delete a disabled tenant's data from the database (`--database-url`, or `DATABASE_URL`), then its lines in the file. Asks for the tenant's name unless given `--yes`, and waits about a minute first, so every hub has closed the tenant. Can't be undone |

- **The hub re-reads the file every second,** so none of these needs a restart.
  A bad line stops the hub at startup; later it's skipped with a warning, so a
  typo locks out only that line.
- **Run `ctxone-hub tenant` as the user the hub runs as.** The first `add`
  creates the file readable by its owner only (`0600`). Edits take
  `<file>.lock` and replace the file with a rename, so its directory must be
  writable, which a read-only mount (a Kubernetes secret, say) isn't.
- **Several tokens for one tenant** (one per device, say): `add` and `rotate`
  give a tenant one, and more are lines you add yourself,
  `alice <sha256 of the token>` with the hash in lowercase hex. `rotate`
  replaces them all.
- Tenant names are 1 to 63 of `a-z`, `0-9` and `-`.

### Run the hub

```bash
export DATABASE_URL='postgres://ctxone:…@db.internal:5432/ctxone?sslmode=verify-full'
ctxone-hub --http --lens --port 3001 \
  --tenants-file /var/lib/ctxone/tenants \
  --strict-origin --allowed-origin https://hub.example.com
```

With `DATABASE_URL` set, the hub uses Postgres; `--storage postgres
--database-url <URL>` says the same. At startup it logs that it's in
multi-tenant mode, with the number of tenants and tokens, what's off, and
`Storage: postgres, every tenant on one pool` with the TLS mode and pool size.
Request log lines carry the tenant as `ctxone.tenant`, never a token.

It refuses to start (exit 64), rather than ignore them, with settings that can't
apply: `--auth-token` or `--auth-token-file` (the tenants file replaces them),
`--tenant`, SQLite, no `--http`, the ASD flags, `--ctx-binary`, and `--lens`
with `--strict-origin` but no `--allowed-origin`. A tenants file it can't read,
or one with a bad line, stops it with exit 66.

As a systemd service, with `DATABASE_URL=…` in `/etc/ctxone/hub.env` and a
`ctxone` user that owns `/var/lib/ctxone`:

```ini
[Unit]
Description=CTXone Hub (multi-tenant)
After=network-online.target
Wants=network-online.target

[Service]
User=ctxone
EnvironmentFile=/etc/ctxone/hub.env
ExecStart=/usr/local/bin/ctxone-hub --http --lens --port 3001 --tenants-file /var/lib/ctxone/tenants --strict-origin --allowed-origin https://hub.example.com
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

Stopping the hub (SIGTERM) saves every open tenant's session statistics first.

### Size the connection pool

Every tenant shares one pool of `--pg-pool-size` connections (default 32, or
`CTXONE_PG_POOL_SIZE`). A request holds a connection only while it reads or
writes, and a closed tenant holds none, so the pool tracks how many requests
run at once, not how many tenants there are. Connections open as they're
needed, up to the limit.

The hub holds at most `--pg-pool-size` connections, plus one while a tenant
opens: tenants open one at a time, each under a lock on a connection of its
own. Keep that, plus a couple for `ctxone-hub tenant purge` and whatever else
uses the database, under the server's `max_connections` (100 by default on
Postgres; managed plans often allow fewer). Lower the pool for a small plan.

### HTTPS in front

The hub listens on `0.0.0.0:<port>` over plain HTTP. Terminate TLS for your
hostname at a reverse proxy or ingress, and keep the hub's port off the
internet. Then:

- **Tokens never cross the network in the clear,** and Lens keeps its token
  for the `https://` origin.
- **There's no loopback exemption** on a multi-tenant hub, so a proxy on
  127.0.0.1 doesn't let requests past the token check, and
  `--require-auth-on-loopback` isn't needed.
- **`--strict-origin --allowed-origin https://<your hostname>`** lets browsers
  call the API only from the hub's own pages. Behind a proxy, `Host` is
  whatever the caller sent, so without `--strict-origin` a matching `Host`
  would be enough.
- **Rate limits are per tenant** (`--rate-limit-rpm`, 600 a minute by default),
  so a proxy that hides client addresses changes nothing.
- **MCP responses can be long-lived event streams.** Don't buffer `/mcp`, and
  allow long requests (nginx: `proxy_buffering off;` and a long
  `proxy_read_timeout`).
- **`/api/health` needs no token,** so a load balancer, a Kubernetes probe or
  the container image's built-in health check can call it as it is. It answers
  only that the hub is up, opens no tenant, and isn't rate limited. A token
  sent with it is still checked, so a wrong one is a 401.

### Connect clients

Each person uses their own token, against the one hostname:

```bash
export CTX_SERVER=https://hub.example.com
export CTX_TOKEN=<your token>
ctx init --mcp-url https://hub.example.com/mcp --auth-token-env CTX_TOKEN
```

- **The `ctx` CLI** reads `CTX_SERVER` and `CTX_TOKEN`.
- **AI tools:** `ctx init` points each one at `/mcp` with the token, here read
  from `CTX_TOKEN` when the tool runs. Claude Desktop's bridge can't read a
  variable, so it needs `--auth-token <token>` instead, written into its
  config ([Authentication](#authentication)).
- **Lens:** open `https://hub.example.com` and paste the token once. Lens keeps
  it in that browser, for that origin; *Forget token* in the sidebar removes
  it.
- **Project detection** needs nothing more: `ctx` sends what it finds in the
  directory, and the hub never reads its own disk for it.
- **Sessions:** `ctx ingest-session` and the Stop hook send a session's
  metadata and extracted memories, but not its raw turns, to a hub that isn't
  on the same machine, unless `remote_turns` is set (see
  [Authentication](#authentication)).

### What's off

A multi-tenant hub's own machine isn't any tenant's, so what acts on it is off:

- ASD code intelligence: the `/api/code` proxy and the MCP code tools;
- discovering, importing, autosyncing and syncing session transcripts from the
  hub machine's disk;
- SQLite backups and the lockfile (back up Postgres instead);
- project detection reading the hub machine's disk;
- `help` running other tools' binaries.

Their routes answer 404 ([HTTP_API.md](HTTP_API.md#multi-tenant-mode)), and Lens
leaves those views out. A single-tenant hub keeps all of them.

### Running it

- **Backups:** a Postgres backup (`pg_dump`, or your provider's snapshots)
  holds every tenant. There's no export of one tenant yet.
- **A lost or leaked token:** `ctxone-hub tenant rotate <name>`.
- **Someone leaves:** `disable` them, and `purge` once their data can go.
- **Upgrades:** stop the hub, which saves every open tenant's statistics, and
  start the new one. Each tenant's schema is brought up to date on its first
  request after that.
- **Idle tenants** close after 30 minutes without a request, and open again on
  the next one. An open tenant's session statistics are saved every 30
  seconds.

## Running as a service

To make the HTTP/Lens hub survive reboots and start **before** any agent (so it,
not an agent's stdio child, owns the db), run it as a login service. `ctx service`
generates and registers the unit for you — launchd on macOS, a systemd **user**
unit on Linux, and a **Task Scheduler** logon task on Windows (via `schtasks`):

```bash
ctx service install                       # lens on, port 3001, canonical db
ctx service install --port 3001 --path ~/.ctxone/memory.db
ctx service install --auth-token "$(openssl rand -hex 32)"   # embeds token (chmod 600)
ctx service install --dry-run             # print the unit + commands, write nothing
ctx service status                        # is it registered/running?
ctx service uninstall                     # stop + remove
```

The unit runs `ctxone-hub --http --lens --path <db> --port <port>` with
`RunAtLoad`/`KeepAlive` (macOS), `Restart=on-failure` (systemd), or a
`LogonTrigger` + `RestartOnFailure` (Windows), logging to `~/.ctxone/hub.log`.
Preview it first with `--dry-run`. If another hub already holds the db (an
agent's stdio hub, or a manual one), the service will fail on the lockfile —
stop it first. On Windows the auth token can't be embedded in the task; set it
with `setx CTXONE_AUTH_TOKEN <token>` instead.

<details><summary>Equivalent hand-written launchd plist</summary>

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.ctxone.hub</string>
  <key>ProgramArguments</key>
  <array>
    <string>/opt/homebrew/bin/ctxone-hub</string>
    <string>--http</string>
    <string>--lens</string>
    <string>--path</string>
    <string>/Users/user/.ctxone/memory.db</string>
  </array>
  <key>KeepAlive</key><true/>
  <key>RunAtLoad</key><true/>
  <key>StandardErrorPath</key><string>/Users/user/.ctxone/hub.log</string>
</dict>
</plist>
```

```bash
launchctl load -w ~/Library/LaunchAgents/com.ctxone.hub.plist
```
</details>

> With the daemon running, point agents at it with `ctx init --transport http`
> and they stop spawning their own hub — one process, one owner, no race.
> `ctx init` handles every detected client (native URL, Codex `url`, or the
> mcp-remote bridge for Claude Desktop), so they all connect to the daemon. Use a
> **separate db** (topology C) only for deliberate isolation, or for a client that
> supports neither native http nor the mcp-remote bridge. See
> [design/UNIFIED_HUB.md](design/UNIFIED_HUB.md).

## Troubleshooting

- **`database is already locked …`** — another hub owns that db. Find it
  (`ps aux | grep ctxone-hub`), or use a different `--path`/`--port`. Stale locks
  (owner gone) are reclaimed automatically.
- **Lens shows nothing / CLI can't connect** — check `CTX_SERVER` and that a hub
  is running with `--http`. `curl http://localhost:3001/api/health`.
- **Agent and Lens show different memory** — they're on different db paths
  (topology C). That's isolation working as configured.
- **Every request to a multi-tenant hub is a 401** — there's no loopback
  exemption, so each request but `GET /api/health` needs a tenant's token.
  `ctxone-hub tenant list` shows whether the tenant is disabled; `rotate` gives
  it a new token.

See also: [ARCHITECTURE.md](ARCHITECTURE.md) · [DATA_SAFETY.md](DATA_SAFETY.md) ·
[TROUBLESHOOTING.md](TROUBLESHOOTING.md)

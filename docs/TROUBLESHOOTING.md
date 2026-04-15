# Troubleshooting

Top errors you'll hit and how to fix them. Run `ctx doctor` first — it
catches most of these automatically and suggests fixes.

## 1. `Hub unreachable (http://localhost:3001)`

**Symptom:** any `ctx` command that talks to the Hub fails immediately.
Exit code 69.

**Cause:** the Hub isn't running, isn't on the port you think it is, or is
bound to a different interface.

**Fix:**

```bash
ctx serve --http           # start it
# ... or in another terminal / systemd service / docker
ctx status                 # verify
```

If you have the Hub running on a different port or host:

```bash
export CTX_SERVER=http://my-hub:3001
ctx status
```

Or pass `--server` explicitly.

## 2. `ctx: command not found`

**Cause (macOS / Linux):** `~/.local/bin` isn't on your `PATH`, or the
install script didn't run to completion.

**Fix (macOS / Linux):**

```bash
export PATH="$HOME/.local/bin:$PATH"
# add the above to your ~/.zshrc or ~/.bashrc
```

Verify the binary exists:

```bash
ls -la ~/.local/bin/ctx
```

**Cause (Windows):** PATH changes made by the installer only take effect
in **new** PowerShell windows. Your current shell still has the old PATH.

**Fix (Windows):** close your PowerShell window and open a fresh one.
Or manually add to the current session:

```powershell
$env:Path += ";$env:LOCALAPPDATA\ctxone\bin"
```

Verify the binary exists:

```powershell
Get-Item "$env:LOCALAPPDATA\ctxone\bin\ctx.exe"
```

If missing, re-run the installer:

```bash
curl -sSL https://raw.githubusercontent.com/ctxone/ctxone/main/install.sh | sh
```

## 3. `No memories found for '<topic>'` but you know the fact is there

**Cause A — wrong branch.** You're reading from `main` but the fact was
written to another branch.

```bash
ctx branches             # see which branch has your fact
ctx --branch <name> recall "<topic>"
```

**Cause B — search vs recall mismatch.** `ctx recall` tokenizes the query
and drops stopwords. "the status" becomes `["status"]` since "the" is a
stopword. Try a more specific single word or use `ctx search` which does
literal substring matching.

**Cause C — the fact is on a pinned path.** Pinned memories are split into
`/title` and `/body` fields. `recall` dedups these, but `search` doesn't —
check both:

```bash
ctx search "<term>"            # literal
ctx ls /memory/pinned          # see pinned paths
```

**Cause D — the fact was forgotten.** Check the commit history:

```bash
ctx log -n 100 | grep -i "<term>"
```

If it was deleted, you can still see the old commit in `blame` but the
current state won't have it.

## 4. `ctx init` wrote a config but Claude Code / Cursor still doesn't see CTXone

**Cause A — the AI tool needs a restart.** Most MCP clients load config on
startup. Restart Claude Code / Cursor / VS Code after running `ctx init`.

**Cause B — wrong config scope.** `ctx init` writes project-level configs by
default (`.mcp.json` in cwd). If you want global, add `--global`.

**Cause C — path mismatch.** Check where `ctx init` actually wrote:

```bash
ctx init --dry-run
```

Copy the path. Open it in an editor. Verify the `mcpServers.ctxone` entry
points at your actual `ctxone-hub` binary.

**Cause D — the Hub binary moved.** If you reinstalled, the old `.mcp.json`
may point at a stale path. Re-run `ctx init` to refresh.

## 5. Ratio stuck near 1.0x — no savings

**Cause A — graph is tiny.** With 5 facts total and recalling 4 of them,
flat ≈ sent. Savings don't kick in until the graph is bigger than what any
given recall returns.

**Cause B — overly broad recall.** `ctx recall "project"` on a
project-heavy graph matches every fact. Try a more specific query.

**Cause C — too much pinned content.** If you've pinned several long docs,
pinned content alone eats the budget. Review with `ctx pinned` and unpin
(via `ctx forget`) anything that's not actually critical.

See [TOKEN_SAVINGS.md](TOKEN_SAVINGS.md) for the full breakdown.

## 6. `branch not found: <name>` when writing

**Cause:** you tried to write to a branch that doesn't exist yet. Branches
must be created explicitly.

**Fix:**

```bash
ctx branch <name>                 # create it
ctx --branch <name> remember "..." # now writes work
```

Or create it from a specific ref:

```bash
ctx branch <name> --from main
```

## 7. `ctx prime` reports "No sections found in <file>"

**Cause:** the markdown file has no H1 or H2 headings. `ctx prime` only
splits at `# ` and `## ` (not `###` or deeper).

**Fix:** either add headings, or accept that the whole file becomes one
"Intro" section.

If you want deeper headings to count, open an issue — we can extend the
parser.

## 8. Cargo build failing with `thiserror not found in workspace.dependencies`

**Cause:** you ran `cargo build` from the root without initializing the
`engine/` git submodule.

**Fix:**

```bash
git submodule update --init --recursive
cargo build --workspace
```

Or clone recursively from the start:

```bash
git clone --recursive https://github.com/ctxone/ctxone.git
```

## 9. `ctx tail` shows nothing when I'm writing in another terminal

**Cause A — wrong branch.** `ctx tail` reads the branch you specified (or
`main` by default). If the writes are going to a different branch, tail won't
see them.

```bash
ctx tail --branch <other_branch>
```

**Cause B — polling interval.** Default is 2000ms. Writes within that window
show up on the next poll. Lower with `--interval 500`.

**Cause C — the write failed.** Check the exit code of the `ctx remember`
command in the other terminal.

## 10. Postgres Hub errors: `database "ctxone" does not exist`

**Cause:** the Postgres database itself isn't created. CTXone creates the
*schema* on init but expects the database to already exist.

**Fix:**

```sql
-- Connect to postgres as a superuser
CREATE DATABASE ctxone;
CREATE USER ctxone WITH PASSWORD 'secret';
GRANT ALL PRIVILEGES ON DATABASE ctxone TO ctxone;
```

Then:

```bash
export DATABASE_URL=postgres://ctxone:secret@localhost:5432/ctxone
ctx serve --http --storage postgres
```

The Hub will create its tables on first run.

---

## Schema migrations

CTXone tracks its own **schema version** separately from the underlying
storage (SQLite / Postgres / in-memory). Whenever we change the *shape*
of what CTXone writes to the graph — path conventions, structured field
layouts, session formats — the schema version is bumped and migrations
run on Hub startup.

### What happens on startup

The Hub reads `/ctxone/schema_version` from the graph and compares it to
the version baked into the running binary.

- **Fresh graph:** writes the current version, no data to migrate.
- **Graph at current version:** no-op (logged at `debug`).
- **Graph behind current version:** runs each pending migration in order
  and bumps the recorded version. Each migration logs at `info`.
- **Graph ahead of current version:** Hub refuses to start. This protects
  you from silent data corruption if you downgrade `ctxone-hub` but keep
  an old graph.

Example startup logs for a fresh install:

```
INFO ctxone_hub::migrations: fresh graph; initializing schema version to=1
INFO ctxone_hub::migrations: migration 001: initialize schema version=1
INFO ctxone_hub::migrations: migrations complete version=1
```

And for a re-boot against the same graph:

```
DEBUG ctxone_hub::migrations: schema version is current, no migrations needed version=1
```

### Hub refuses to start: "graph schema version X is newer than this Hub"

You downgraded `ctxone-hub` but kept a graph written by a newer version.
Options:

1. **Upgrade back:** re-run `install.sh` or `install.ps1` to get the
   latest Hub, or `docker pull ghcr.io/ctxone/ctxone:latest`.
2. **Start fresh:** delete `~/.ctxone/memory.db` (or `%APPDATA%\ctxone\memory.db`
   on Windows) and re-run. You lose the graph but get a clean start on
   the old binary.
3. **Keep both:** point the older Hub at a different `--path` so the
   two graphs don't interfere.

### Inspecting the schema version

```bash
ctx get /ctxone/schema_version
```

Outputs the integer version. The write is recorded under the
`ctxone-migration` agent with a `Migrate` intent, so `ctx blame
/ctxone/schema_version` shows exactly when each version bump happened.

## Enabling verbose logs

The Hub uses the `tracing` crate. Set `RUST_LOG` before starting it to
control verbosity:

```bash
# Default — info-level startup and recall telemetry
ctx serve --http

# Debug — also see prime/forget/remember request details
RUST_LOG=debug ctx serve --http

# Trace — every field of every span, useful for deep debugging
RUST_LOG=trace ctx serve --http

# Scoped — only enable debug for CTXone's own code
RUST_LOG=ctxone_hub=debug ctx serve --http

# Combined — debug CTXone, info HTTP request traces from tower-http
RUST_LOG=ctxone_hub=debug,tower_http=info ctx serve --http
```

All logs go to **stderr**, so they never corrupt the stdio MCP channel
when the Hub runs as an MCP server.

In HTTP mode, every `recall` call emits an `info`-level line with the
topic, tokens sent, and savings ratio — useful for watching memory earn
its keep in real time. Writes log at `debug` level.

## Rate limiting

The Hub enforces a per-peer-IP token-bucket rate limit when running in
HTTP mode. The default is **600 requests per minute per IP**, which is
permissive enough that real agents and even heavy `ctx prime` imports
stay well under the limit, but catches runaway loops and abuse.

When a client exceeds the limit, the Hub returns:

```
HTTP/1.1 429 Too Many Requests
Retry-After: <seconds>
X-RateLimit-*: <headers>
```

Well-behaved clients should honor `Retry-After`. The Python client
(`ctxone`) does this via `requests`, and the Rust CLI falls through
to the normal error handler.

### Changing the limit

**CLI flag** (highest priority):

```bash
ctxone-hub --http --rate-limit-rpm 120    # 120 req/min per IP
ctxone-hub --http --rate-limit-rpm 0      # disable entirely
```

**Environment variable** (used when no flag is given):

```bash
CTXONE_RATE_LIMIT_RPM=60 ctxone-hub --http
```

`0` disables rate limiting entirely — useful in trusted single-tenant
deployments behind their own network ACLs, or when running in tests.

### "I'm getting 429s on legitimate traffic"

Usually it's one of three things:

1. **Behind a reverse proxy that collapses client IPs to the proxy's
   own IP.** The rate limiter sees every request as coming from one
   client and throttles hard. Configure the proxy to forward the real
   client IP via `X-Forwarded-For` (a future release will add support
   for trusted forwarded headers — today you can disable rate limiting
   and let the proxy handle it).
2. **Running a parallel batch job** from a single IP. Bump the limit
   with `--rate-limit-rpm 3000` or disable it with `--rate-limit-rpm 0`
   during the batch.
3. **Tests that hammer the endpoint.** The library default
   (`HubConfig::default()`) is `0` specifically so in-process tests
   don't get throttled. If you're writing your own Rust tests, build
   the router via `http::router_with_config(..., HubConfig { rate_limit_rpm: 0 })`.

## Per-session token tracking

The Hub tracks tokens-used and tokens-saved **per session** when
clients send the `X-CTXone-Session` header. Without the header, all
usage rolls up under the `"default"` session.

### Endpoints

| Endpoint | What it returns |
|---|---|
| `GET /api/stats/tokens` | Aggregate roll-up across every session (backward compat) |
| `GET /api/stats/tokens/{session_id}` | Stats for a specific session, 404 if unknown |
| `GET /api/stats/sessions` | List every known session with its stats |

The aggregate endpoint uses `"_aggregate"` as the session ID in its
response body so it's unambiguous that you're looking at a roll-up.
Graph size (`total_graph_size_chars`) is **not** summed across
sessions — it's the same graph for everyone, so the aggregate takes
the maximum observed value.

### Setting the session ID

**Python client:**

```python
from ctxone import Hub

hub = Hub(session_id="alice@example.com")
hub.recall("licensing")  # counts toward alice's session
```

Or via environment:

```bash
export CTX_SESSION_ID=alice@example.com
python my-agent.py
```

**Raw HTTP:**

```bash
curl -H "X-CTXone-Session: alice@example.com" \
     "http://localhost:3001/api/memory/recall?topic=licensing"
```

**Open WebUI plugin:** the `ctxone.integrations.openwebui` Tool and
Filter automatically set `X-CTXone-Session` to the Open WebUI user's
email (or name/id), so a multi-user self-hosted install gets per-user
stats for free.

### "My session isn't showing up in /api/stats/sessions"

Sessions are created lazily the first time a request with that header
arrives at a read endpoint that records usage (`recall`, `context`).
Write endpoints (`remember`, `forget`, `prime`) don't record tokens,
so writing to a new session ID without ever reading from it leaves
the session invisible to the registry. Do one `recall` to materialize it.

## Per-tool agent IDs

`ctx blame` shows a "who" column for every fact in the graph — the
agent ID that was stamped on the commit. By default that's
`"ctxone"`, which is useful for "this came from CTXone" but tells
you nothing about *which* tool wrote it. T2 makes agent IDs
per-tool so you can tell Claude Code's writes from Cursor's from
Open WebUI's.

### How the agent ID is resolved

Highest priority wins:

1. **HTTP request**: `X-CTXone-Agent: <name>` header on the request
2. **MCP stdio**: `--agent-id <name>` flag passed when spawning
   `ctxone-hub` as a subprocess (or `CTX_AGENT_ID` env var)
3. **Fallback**: `"ctxone"`

### Setting the agent ID from clients

**Python client:**

```python
from ctxone import Hub
hub = Hub(agent_id="my-script")     # or CTX_AGENT_ID env var
hub.remember("...")                 # blame shows "my-script"
```

**Raw HTTP:**

```bash
curl -H "X-CTXone-Agent: my-script" \
     -H "Content-Type: application/json" \
     -d '{"fact":"..."}' \
     http://localhost:3001/api/memory/remember
```

**MCP stdio (what AI coding tools use):** `ctx init` writes
MCP config files like `.mcp.json` and `.cursor/mcp.json` with
`--agent-id <slug>` already baked in — Claude Code writes show up
as `claude-code`, Cursor as `cursor`, VS Code as `vs-code`, etc.
If you have an old config that predates T2, re-run `ctx init` to
upgrade it.

**Open WebUI plugin:** the bundled Tool and Filter automatically
set both `X-CTXone-Agent` and `X-CTXone-Session` to the user's
email (or name/id/fallback), so a multi-user install gets per-user
attribution for free.

### "I ran `ctx init` a while ago and blame still shows `ctxone`"

Your MCP configs predate T2. Re-run:

```bash
ctx init
```

which will rewrite `.mcp.json` / `.cursor/mcp.json` / `.vscode/mcp.json`
/ Codex's `config.toml` etc. with `--agent-id <tool-slug>` in the
args, then restart your AI tool so it picks up the new config.

## Still stuck?

- Run `ctx doctor` — it catches most infrastructure problems automatically.
- Check the Hub logs. If running via `ctx serve`, errors print to stderr in
  that terminal. Use `RUST_LOG=debug` for more detail.
- Open an issue at https://github.com/ctxone/ctxone/issues with: what you
  tried, what you expected, what you got, and the output of `ctx --version`
  and `ctx doctor`.

# Changelog

All notable changes to CTXone are documented here. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning follows
the project's release tags (`v1.0.x`, incremented by 0.0.1 per release).

## [Unreleased]

### Added
- **Tenant identity for a multi-tenant Hub,** the first part of one Hub serving many tenants (docs/design/MULTI_TENANT_HUB.md).
  - The tenants file has a line per token, `<tenant> <sha256 of the token>`, and a tenant may have several. Only hashes are stored. It's re-read every second, so tenants and tokens change without a restart. A bad line stops the Hub at startup; later it's skipped with a warning, so a typo locks out only that line. A hash listed for two tenants is refused for both.
  - Every request needs a token that belongs to a tenant, loopback included. An unknown token is a 401. The hash of the presented token is compared in constant time. A `TenantResolver` trait decides the tenant, so an account service can replace the file; one that can't answer gives a 503.
  - The tenant is attached to the request. Rate limits are per tenant instead of per peer IP. Request log lines carry `ctxone.tenant`, never the token. `GET /api/tenant` reports the request's tenant (`null` on a single-tenant Hub).

- **Each tenant's own data on a multi-tenant Hub,** the second part.
  - A request reaches only its tenant's repository, project registry, session statistics and MCP sessions. Handlers get them from `HubState`, now built for each request from the request's tenant, so on a multi-tenant Hub nothing reaches data except through it. Tenants can use the same workspace names, project ids and session ids.
  - A tenant opens on its first request that needs its data. On Postgres every tenant shares one connection pool and the `ctxone_*` tables. Opening a tenant creates it if it's new, runs the AgentStateGraph and CTXone migrations for it and loads its session statistics, holding an advisory lock so two Hubs opening the same tenant take turns. A tenant that can't be opened gives a 503, and the next request tries again.
  - An idle tenant is closed after its statistics are flushed (`TenantCache::evict_idle`), never while a request is using it.
  - The Hub Home's workspace summary, reused for 15 seconds, is kept per tenant. It was process-wide, so one tenant could have been served another's.
  - For library users: `http::multi_tenant_router(config, MultiTenant { resolver, data })` builds a multi-tenant Hub and replaces `HubConfig::tenants`. The new `tenant_cache` module has `TenantCache`, the `TenantOpener` trait, `PostgresTenants` and `MemoryTenants`. `migrations::migrate_asg` is the AgentStateGraph schema step that startup and each tenant's open share. `deadpool-postgres` 0.14, already in the build through AgentStateGraph, is now a direct dependency.

- **A multi-tenant Hub serves,** the third part. `ctxone-hub --http --tenants-file <PATH>` with Postgres (`--storage postgres` and `--database-url`, or `DATABASE_URL`) serves every tenant from one process and one connection pool; `--storage memory` keeps the tenants in memory, to try it out.
  - The open tenants' session statistics are flushed every 30 seconds, and a tenant with no request for 30 minutes is closed after a flush (`TenantCache::spawn_upkeep`). Stopping the Hub flushes every open tenant.
  - Nothing acts on the Hub's own machine, whose disk, transcripts and programs aren't the tenants'. Session sync, import and autosync, the admin backup and the ASD code proxy answer 404. Project detection needs `client_read=true`, which `ctx` sends, and is a 400 without it. Help asks no other tool's binary. The MCP code tools, which run the `asd` CLI or reach ASD repos, say there's nothing to reach. Flags for those features are refused at startup, and the Hub logs what's off. A single-tenant Hub is unchanged.
  - The graph-size refresher doesn't run, since its one number for the process can't be every tenant's; each session computes its tenant's, throttled.

- **Isolation tests across every route and MCP tool,** the fourth part. Two tenants hold data under the same names: workspace, project id and remote, session ids, plans, branch, document and memory contexts. One then calls every REST route and MCP tool with those names and with the other's ids (fact paths, commit, reminder, taint and draft ids, its MCP session), and must get none of its data, open none of it and change none of it. A route or tool added without a probe fails the test. It runs on Postgres in CI. Two things it found shared across tenants are now each tenant's own:
  - The recall result cache was one for the process, keyed by graph state without a tenant. A cached result could only match the tenant whose graph made it, since the key's commit id is a hash of that graph, but nothing else kept it there. Each tenant has its own now, and the key also names the workspace.
  - The "request has no namespace" warning was deduplicated in a set of request paths, which carry the plan names and session ids clients choose: any client could grow it without bound, and on a multi-tenant Hub one tenant's requests muted the warning for others. It keeps routes now.

- **`ctxone-hub tenant` administers a multi-tenant Hub's tenants,** the fifth part. `add <name>` creates a tenant and prints its token once (`ctx_` and 64 hex digits; only the hash is kept). `list` shows the tenants and their tokens, `rotate <name>` replaces a tenant's tokens with a new one, `disable <name>` and `enable <name>` refuse or accept its tokens and keep its data, and `purge <name>` deletes a disabled tenant's data, then its tokens. Each takes `--tenants-file` (or `CTXONE_TENANTS_FILE`); purge also takes `--database-url` (or `DATABASE_URL`).
  - The tenants file gains `disabled <tenant> <sha256 of a token>` lines: the token is kept and refused. A hash listed for two tenants is still refused for both when one of the lines is disabled.
  - Edits keep the file's comments and its other lines, wait for each other on `<file>.lock`, and replace the file with one rename, so a Hub never reads half of it. A new file is readable by its owner only; an existing one keeps its permissions. A file with a line a Hub can't use is left alone until it's fixed, an edit that changes nothing writes nothing, and only `add` makes a missing file.
  - A Hub closes a tenant at its next upkeep pass, within 30 seconds, once no token of the tenant's is accepted, after flushing it (`TenantResolver::admits`, `TenantCache::close_refused`). Until then a disabled tenant stayed open for 30 minutes and its statistics were written every 30 seconds.
  - `purge` takes only a tenant the tenants file lists with every token disabled, so a mistyped `--tenants-file` can't delete a tenant in use, and asks for the tenant's name unless given `--yes` (with no terminal to ask on, it refuses). It then waits 61 seconds, so every Hub has closed the tenant and written its statistics for the last time, and deletes the tenant's rows from every AgentStateGraph and `ctxone_*` table in one transaction, holding the lock under which a Hub opens the tenant. A database with a table that has a `tenant_id` column the Hub doesn't know is refused, and nothing is deleted. Then the tenant's lines leave the file, so the name can be added again with nothing in it.
  - `getrandom` 0.3, already in the build, is now a direct dependency, for the tokens.

- **Lens on a multi-tenant Hub,** the sixth part. `--lens` works with `--tenants-file`. Lens asks for a tenant's token once, keeps it in the browser's storage for the Hub's origin, and sends it as a bearer token on every call, the Code views' included. Lens's own files need no token; its data does.
  - A 401 opens token entry, which checks a token with `GET /api/tenant` before keeping it. A refused token is forgotten, so a rotated or disabled one brings token entry back. The sidebar names the tenant and can forget the token.
  - With `--lens` and `--strict-origin`, a multi-tenant Hub won't start without `--allowed-origin`: Lens calls from the Hub's own origin, which `--strict-origin` doesn't trust by `Host`, so it has to be listed.
  - The same token entry serves a single-tenant Hub with a token, reached from another machine, which needed an SSH tunnel until now. A local Hub never asks.
  - Lens leaves out what a multi-tenant Hub turns off: the Code views (from the sidebar and the command palette, and a link to one says there's no code intelligence on this Hub), and session import, autosync and transcript sync on the Sessions page. It asks `GET /api/tenant` on every load which kind of Hub it's on: only a multi-tenant Hub names a tenant. A single-tenant Hub shows everything, as before.

- **`--pg-pool-size <N>`** (or `CTXONE_PG_POOL_SIZE`) caps the Postgres connections the Hub holds, 32 by default. A multi-tenant Hub's tenants share them.

- **Row-level security on a Hub's Postgres database,** a backstop behind every query's tenant filter. `ctxone-hub db row-security on|off|status` (`--database-url`, or `DATABASE_URL`) switches it for every table that holds tenants' rows, AgentStateGraph's and the Hub's `ctxone_*` tables, or prints whether it's `on`, `off` or `partial`. It's off until switched on, once per database, and running Hubs needn't restart. With it on, a statement sees and writes only its connection's tenant's rows.
  - The Hub gives its `ctxone_*` tables AgentStateGraph's tenant policy where it sets them up, so existing tables get it from the first Hub to start (or from `ctxone-hub db`).
  - Every connection the Hub uses names its tenant: the project registry, session statistics, the lock a tenant opens under, and `ctxone-hub tenant purge`, which deletes as the tenant it purges.
  - At startup a Hub on Postgres logs whether it's on, and warns when it's on but the Hub's role is a superuser or has `BYPASSRLS` (both skip it), when it's on for only some tables, or when a table with a `tenant_id` column has no policy.
  - The isolation sweep and the purge test also run with it on, as a role it binds.

### Changed
- **AgentStateGraph 1.5.0** (`agentstategraph*` = "1.5.0", from 1.4.0). Its `PostgresStorage::for_tenant` (1.4.2) opens another tenant on the same connection pool, which the multi-tenant Hub's per-tenant state builds on. 1.5.0 adds the row-level security policies and `PostgresStorage::tenant_pool`, a pool whose connections name their tenant, which the Hub now uses for its own tables in place of the deprecated `pool()`. Naming the tenant takes one more round trip per checkout. `Cargo.lock` moves only the seven agentstategraph crates.
- **`GET /api/health` needs no token,** on any Hub, so a load balancer, a Kubernetes probe or the container image's own health check can call it. Its answer is fixed and it opens no tenant. It isn't rate limited either, so probes neither use up a tenant's or an address's budget nor get refused for one. A token sent with it is still checked, so a wrong one is a 401. Before, a multi-tenant Hub refused a health check without a tenant's token, which failed the image's health check, and so did a Hub with `--require-auth-on-loopback`.

### Fixed
- **An out-of-order `main` pipeline no longer fails its GitHub publish.** When a newer commit's pipeline had already published `main`, `publish-github` treated GitHub being ahead as divergence and rescanned all of history, though no commit was about to become public, and the job failed with nothing to push. It now skips the history scan in that case (it still scans the working tree) and skips the push as before.
- **The rate limiter no longer keeps a bucket for every loopback request.** On a single-tenant Hub, each loopback request (local agents, the `ctx` CLI, session sync) got a bucket of its own so it would never be limited, and nothing ever dropped a bucket. The Hub's memory grew with its local traffic: 50 loopback requests left 50 buckets. Loopback requests now skip the limiter and leave nothing behind. A thread drops the buckets that have refilled every minute, so an address that stops calling is forgotten too. The limits themselves are unchanged.
- **MCP over HTTP names the workspace it serves.** A session on `/mcp?namespace=<ns>` (or with `X-CTXone-Namespace`) read and wrote `<ns>`, but its tools said `default`. `project_status`, `remember`, `summarize_session` and `session_info` reported the `default` workspace. Closing a plan (`plan_close`, `plan_complete`, or `plan_done` on its last task) sealed its epoch as `plan:default:<plan>`. Workspaces closing plans of the same name collided on that id, so only the first got an epoch, and the epoch listed under its raw id instead of the plan's name. MCP over stdio and the REST API were right.
  - Epochs sealed this way keep their ids. `POST /api/plans/backfill_epochs` in the plan's workspace seals the plan again as `plan:<ns>:<plan>`; the old epoch stays, listed in that workspace under its raw id. While it's there, a plan of the same name in the `default` workspace can't seal its own epoch, since AgentStateGraph can't delete an epoch.

### Docs
- **docs/DEPLOYMENT.md covers one Hub for many users:**
  - what it takes (Postgres, one hostname behind HTTPS, one Hub process);
  - adding, rotating, disabling and purging tenants with `ctxone-hub tenant`, and who may edit the tenants file;
  - running the Hub, with a systemd unit and the settings it refuses;
  - sizing `--pg-pool-size` against Postgres's `max_connections`;
  - what a proxy in front needs (no buffering for `/mcp`);
  - connecting `ctx`, AI tools and Lens with a tenant's token;
  - what's off, and backups, upgrades and offboarding.

  The flag and environment tables list `--tenants-file`, `--pg-pool-size`, `--database-url` and their variables. The design doc (docs/design/MULTI_TENANT_HUB.md) is marked implemented.

- **docs/DEPLOYMENT.md covers row-level security:** switching it on, a role it binds for the Hub, running `ctxone-hub db` as the tables' owner, and backing up as a role that skips it.

## [v1.0.15] — 2026-10-21

### Fixed
- **The Docker image builds again.** The Dockerfile built with `rust:1.88-slim`, but the oxtel crates added in v1.0.14 need Rust 1.94, so the v1.0.14 image failed to build and `ghcr.io/agentstatelabs/ctxone` stayed at v1.0.12 (v1.0.13's build had failed on a Docker Hub rate limit). The builder is now `rust:1.96-slim-bookworm`, the Rust version CI uses, on the same Debian release as the runtime image; plain `rust:1.96-slim` is Debian trixie, whose glibc is newer than the bookworm runtime's. docs/WINDOWS.md said a source build needs Rust 1.88; it needs 1.94.

## [v1.0.14] — 2026-10-20

### Added
- **Tasks can be edited after they're added.** It's available as the `plan_edit` MCP tool, `PATCH /api/plans/{name}/tasks/{task_id}` and `ctx plan edit`. It changes a task's title, description, priority or blockers, and leaves its status, proof, assignment and timestamps as they are.
  - Before, revising a plan meant adding new tasks beside the old ones. Abandoning the old ones didn't help, because an abandoned blocker never counts as done, so its dependents stayed blocked; now their blockers can be edited.
  - `blocked_by` replaces the whole list. Each blocker must be a task in the plan, and a list that would form a cycle is refused (409).
  - `title` keeps the task's description, and `description` keeps its title (`""` removes it).
  - Done and abandoned tasks need `force`.
  - The response's `changed` lists what changed. Each changed field is one commit, made as the calling agent.
  - Requires AgentStateGraph 1.4.0 (`agentstategraph*` = "1.4.0") for `TaskStore::set_title` and the cycle check in `set_blockers`. `Cargo.lock` moves only the seven agentstategraph crates.
- **The Hub connects to Postgres over verified TLS.** It builds AgentStateGraph's storage with `postgres-tls` (rustls with the ring backend and Mozilla's roots, crates the Hub already used), so `DATABASE_URL` can ask for `sslmode=verify-full`; a provider with its own CA takes `sslrootcert=<CA file>` or `PGSSLROOTCERT`. Before, any TLS setting was refused with an error naming the feature, so the Hub couldn't reach a managed Postgres that requires TLS. The startup line now says which TLS mode is in use (`tls=verify-full`, never the URL), and a database on another machine with no `sslmode`, or `prefer`, gets a warning; `sslmode=disable` says plaintext is intended. `scripts/test-postgres-tls.sh` runs the TLS tests against a throwaway Postgres that accepts only TLS. See docs/DEPLOYMENT.md, "Postgres over TLS".
- **Logs, traces and metrics through [oxtel](https://github.com/agentstatelabs/oxtel).** The Hub's hand-built `tracing-subscriber` setup is replaced.
  - Logs still go to stderr. They're text on a terminal and one JSON object per line otherwise, so a log file or container gets JSON. `OXTEL_LOG_FORMAT` (`text`, `json`, `ecs`, `none`) chooses. One startup line says where logs go and whether anything is exported.
  - In HTTP mode, each request gets a span named by its route and a `request` log line with its status, duration and trace id. This replaces tower-http's trace layer, which logged at `debug`.
  - MCP requests, over HTTP and stdio, get spans named like `tools/call recall` and `mcp.server.operation.duration`. Tool arguments and results are never recorded.
  - **Nothing is sent anywhere by default.** The new opt-in `otlp` feature sends traces, metrics and logs over OTLP once `OTEL_EXPORTER_OTLP_ENDPOINT` is set. A default build contains no OTLP client.
- **Hosted-mode hardening for the Hub.**
  - `--auth-token-file <PATH>` (`CTXONE_AUTH_TOKEN_FILE`): bearer tokens, one per line, re-read every second, so tokens rotate without a restart. Every listed token is accepted, and an empty file accepts none.
  - `--require-auth-on-loopback` (`CTXONE_REQUIRE_AUTH_ON_LOOPBACK`): loopback peers must send the token too, for a hub behind a proxy or sidecar on 127.0.0.1. The Hub then hands its own `ctx` (session sync and import) the token as `CTX_TOKEN`.
  - `--strict-origin` (`CTXONE_STRICT_ORIGIN`): browsers may call the API only from `--allowed-origin` origins, never because `Origin` matches `Host`.
  - The Hub refuses to start with both token options, with `--require-auth-on-loopback` and no token, or with a token file it can't read.
- **A Postgres Hub has a project registry and keeps its session stats.** With `--storage postgres`, the project endpoints returned 400, project detection reported `registry_unavailable` (so the stdio MCP server fell back to `default`), and session stats were lost at every restart. Both now live in the Hub's database, in `ctxone_projects`, `ctxone_project_paths` and `ctxone_sessions`. Every row carries the Hub's tenant (`--tenant`), so Hubs sharing a database never see each other's projects or stats. The tables use AgentStateGraph's connection pool, so they get its pool size and TLS, and a Hub holds one set of connections. The Hub creates them at startup, under an advisory lock so Hubs starting together don't collide. SQLite Hubs are unchanged.

### Changed
- **The Hub's busiest reads take a handful of queries instead of one or more per item.** On a Postgres Hub each storage read is a round trip, and these endpoints read per item:
  - the session list read each session's title, metadata and turns separately (Lens's Sessions view);
  - the plan list read each plan's tasks;
  - plan show read each task's cross-plan links;
  - pinned memory read each pinned leaf.

  Now the session list reads `/sessions` once two levels deep (titles, turns) plus every session's metadata in one `get_json_many`. The plan list uses `list_plans_with_tasks`, plan show reads the plan's links once, and pinned memory reads its subtree once and flattens it here. Results are unchanged; tests compare the new session discovery and pinned listing with the old reads.

  Measured on a large real store (160 sessions in the registry) against local Postgres, before and after:
  - session list (largest workspace): 3,773 queries (3.9 s) → 198 (0.23 s);
  - plan list: 343 → 10;
  - plan show: 132 → 22;
  - pinned: 315 → 9.

  SQLite got faster too: the session list went from 1.1 s to 0.14 s. Requires AgentStateGraph 1.3.2 (`agentstategraph*` = "1.3.2"), whose tree reads and writes are batched as well; `Cargo.lock` moves only the seven agentstategraph crates.

- **docs/MCP_TOOLS.md documents all 67 MCP tools.** It said 56 (FEATURES.md said 52 in one place and 56 in another), and 13 served tools had no section: `help`, `plan_close`, the curation tools (`session_segments`, `draft_stage`, `drafts_list`, `draft_promote`, `draft_discard`), `session_info`, `session_link_plan`, `namespace_for_plan`, `namespace_for_task`, `code_cross_repo_edges` and `code_impact`. Each now has one, and the summary has two new groups, Curation and Sessions and workspaces. `plan_complete`'s entry said it marked the remaining tasks done; it abandons them and needs a `summary`. A test compares the docs with the tools the server serves, so adding a tool without documenting it now fails the tests.
- **`ctx` reads the directory for project detection, so detection works with a Hub on another machine.** `ctx` sent its cwd to `GET /api/projects/detect` and the Hub looked for `.ctxproject` and ran `git remote get-url origin` there, on its own filesystem. A Hub on another machine (a hosted Hub) can't see that directory, so only a registered `local_path` could match. Now `ctx` reads the nearest `.ctxproject` and the git remote itself and sends them (`client_read=true&ctxproject=…&remote_url=…`, plus `ctxproject_error` for a marker it may not read); the Hub only looks them up and reads nothing. The read is bounded at 3 s, so one blocked by a macOS privacy prompt fails with exit `75` rather than hanging; the prompt is now for the app running `ctx`, usually your terminal. Requests without `client_read` (older `ctx`) are answered as before, and an older Hub ignores the new parameters. Covers every `ctx` command that detects, `ctx status`, `ctx project detect` and `ingest-session --all`.
- **AgentStateGraph 1.3.1** (`agentstategraph*` = "1.3.1"), whose `PostgresStorage` shares its connection pool, so the Hub's own Postgres tables (above) use it.
- **AgentStateGraph 1.3** (`agentstategraph*` = "1.3", from 1.2.8). The Hub's `--storage postgres` mode gets AgentStateGraph's Postgres fixes: storage that works from any caller, working epochs, atomic multi-step writes, history and search on Postgres, and garbage collection. CTXone's own code is unchanged; `Cargo.lock` adds `oxtel-sql`, which AgentStateGraph's storage uses for query telemetry. Postgres TLS (AgentStateGraph's opt-in `postgres-tls` feature) is not enabled yet.
- **LICENSE names AgentStateLabs, LLC as the licensor without the "not yet formed" placeholder, and the Licensed Work as "CTXone" without a version.** The old line named 0.73.0 while shipping with every release; each version already has its own Change Date.
- **LICENSING.md had v0.73.0 converting to Apache 2.0 on 2030-04-15, the old four-year term; it is 2027-10-15.** It also now states that releases before 2026-09-09 (v1.0.10 and earlier) get the 18-month term, and licensing questions go to licensing@agentstatelabs.com, matching LICENSE (the `ctx` CLI's licensing note too).
- Package metadata (Cargo, PyPI, the Open WebUI plugin) credits AgentStateLabs, LLC as the author. The plugin's `author_url` pointed at github.com/ctxone, which does not exist; it now points at github.com/agentstatelabs.
- **Raw session turns stay on this machine unless the hub is here too.** When `ctx`'s hub isn't on this machine (anything but `localhost`, a `.localhost` name, 127.x or `::1`), `ctx ingest-session`, `ctx session import` and the Stop-hook `ctx capture-turn` no longer send the turns; titles, metadata, provenance, token counts and extracted memories still go. An import says so in one line. Opt in with `ctx config set remote_turns true`, or `CTX_REMOTE_TURNS=1` for one command. A hub on this machine, including the Hub's own background import, is unaffected.

## [v1.0.13] — 2026-10-09

### Added
- **CTXone is packaged for crates.io.** `cargo install ctxone ctxone-hub` installs `ctx` and the Hub. The CLI package is renamed from `ctx`, which is taken on crates.io, to `ctxone`; the binary is still `ctx`. The published Hub embeds Lens: `scripts/stage-lens.sh` copies `web/build/` into `server/lens/` before publishing, and `server/build.rs` embeds that copy when `web/build/` is absent. `scripts/publish-crates.sh` runs the publish (dry run by default) and refuses a real upload unless HEAD is the release tag and nothing else is uncommitted. Both crates ship the license and a README.

### Changed
- **AgentStateGraph comes from crates.io** (`agentstategraph*` = "1.2.8") instead of git tags, which crates.io rejects in published manifests.
- **`ctx` embeds AGENTS.md from inside the CLI crate** (`cli/assets/AGENTS.md`), so the published package builds; `include_str!` reached `../../docs/`. `docs/AGENTS.md` stays the source of truth, and a test fails if the two differ. The Docker build no longer copies `docs/AGENTS.md` separately.
- `Cargo.lock` moves `chacha20` from 0.10.0 (yanked) to 0.10.2.
- **`agent-skillgen` is vendored into the CLI** (`cli/src/skillgen/`, from v0.1.0) instead of being a git dependency, which crates.io rejects. AgentStateDeveloper carries the same copy; change it upstream first and re-copy it into both.

## [v1.0.12] — 2026-09-30

### Fixed
- **`ctx` no longer falls back to the `default` workspace when it can't find out which workspace you're in.** The CLI asked the Hub (`GET /api/projects/detect`) with a 1.5 s timeout and treated a timeout, an error or an unreachable Hub exactly like "no project here". After a `brew upgrade`, macOS blocked the new hub's read of `.ctxproject` under `~/Documents` until a privacy prompt was answered, so every command ran against `default` without a word and plans and branches came back "not found". Detection now has three outcomes, not two: found, no project (→ `default`, as before), or failed. A failure stops the command with `can't tell which workspace <dir> belongs to: <why>` and the `--namespace` hint (exit `75` timeout, `76` Hub error, `69` unreachable). `ctx status` and `ctx doctor` report it instead of stopping; `ctx init` continues with a warning only when the Hub isn't running yet; local-only commands (`tokens`, `worktree`) and `ingest-session --all` (which routes by each transcript's own cwd) don't depend on it. The detect request now carries `--token`, so an authenticated Hub's 401 can't be mistaken for "no project".
- **The Hub's detect endpoint times out instead of hanging.** Detection is capped at 3 s and answers `504 timeout`, with a hint about the macOS prompt; the CLI waits 5 s, so it gets that explanation rather than a bare client timeout. A timed-out detection leaves its thread blocked (a thread parked in `open()` can't be cancelled), so once 16 are stuck the Hub refuses new detections instantly with `503 busy` rather than pile up threads. A `.ctxproject` the Hub may not read (a *denied* prompt) and a failed registry query are now `500 error`, not `not_found` / `registry_unavailable`; an unreadable marker still lets the git-remote and local-path steps identify the project.
- **Session sync no longer files sessions in `default`, or registers a duplicate project, when detection fails.** `ctx ingest-session` treated a failed detection as "no project": with auto-registration on, it could mint a second project from the repo's remote, splitting the repo across two workspaces. Those sessions are now skipped for the run and listed with the reason, and the next sync imports them. The `--all` summary line gains `skipped_unresolved`.

### Docs
- `RELEASE.md` documents the post-upgrade macOS privacy prompt and how to clear it. `TROUBLESHOOTING.md` §11 covers the new error; `HTTP_API.md` documents the detect endpoint's error responses and its `local_path` step.

## [v1.0.11] — 2026-09-24

### Fixed
- **`ctx merge` could hang indefinitely and take the whole Hub with it.** On a long-lived workspace the merge base was O(n³) in the shared history: a plan branch with 1,304 common ancestors never finished. Both a `--dry-run` and the real merge were abandoned after five minutes and were still consuming CPU hours later, and because every commit read holds the storage lock, reads and `/api/health` stalled too. The fix is in AgentStateGraph v1.2.4's merge base, now linear: the same branch pair resolves in 65 ms, and an end-to-end `ctx merge --dry-run` on that workspace returns in about a second.
- **Merges no longer run on the async runtime, and no longer stack.** `POST /api/merge` ran the merge synchronously inside its handler, pinning a worker for its whole duration. It now runs on the blocking pool, and only one merge per (workspace, target ref) runs at a time; a concurrent one gets `409 merge_in_progress` with a retry hint instead of piling on. The in-progress marker lives as long as the merge work, not the request — a disconnect drops the request but not the merge. This is deliberately not a timeout: a merge cannot be stopped midway safely, and reporting "failed" while a real merge might still commit a moment later would be worse than waiting.
- **`ctx plan move` could not land a finished plan on a branch that held a stale copy.** It refused any same-named plan on the target and advised "rename or archive the conflicting plan first" — but archiving never cleared the collision and there is no rename. `--replace-archived` (MCP/HTTP `replace_archived`) now replaces the target copy if, and only if, it is archived; an active or completed copy is still refused, and the error names its status. The archived copy is cleared as a whole, so none of its tasks leak into the moved plan, and it stays recoverable in the branch history. Workflow for a stale copy: archive it, then move with `--replace-archived`.
- **`ctx help plan_move` documented the wrong command** — it described `ctx plan relocate` (cross-workspace). It now documents the branch move, and relocate has its own `plan_relocate` entry.

### Changed
- **AgentStateGraph v1.1.2 → v1.2.4.** Beyond the merge base, CTX picks up:
  - `blame`, commit-graph views, stats and the timestamp-anomaly check now walk every parent rather than the first-parent line, so changes made on a merged-in branch are attributed to the commit that made them rather than the merge.
  - The history rollup files commits with no attributable workspace under `unattributed` instead of `default`, which is a real workspace name. Rows distilled before this upgrade keep their old value.
  - Paged commit queries now see merged-in history and no longer return an empty page past a hard-coded scan limit.
  - Checkpoint milestones pin their state only when asked to, and the GC sweep takes the store's write lock. CTX does not run GC itself, so neither changes day-to-day behaviour.

  The Hub opens an existing store unchanged (schema `0.4.0`); verified by starting it against a copy of a 2.3 GB field store, which came up healthy with reads, recall and plans intact.

## [v1.0.10] — 2026-09-02

### Changed
- **A task description can be 8 KiB, up from 2 KiB.** A title is a label; a description is a *brief* — why the task matters, what to exercise, what is most likely to be wrong, what counts as proof. 2048 bytes did not fit that: a real installer-verification task listing four cases and three likely failure modes came to ~2500 bytes and had to be cut down, losing exactly the detail that makes a task actionable months later. The character-level filter (`is_unsafe_text`, which rejects newlines, bidi overrides, zero-width and tag characters) is unchanged and remains the primary injection control — the byte cap only ever stopped someone pasting a whole document, and 8 KiB is still orders of magnitude below that.

  The description boundary had no test at all before this. It now has three, because the obvious two are not enough: at-limit and over-limit both track the constant, so they pass at *any* value and prove only that validation is wired. A third pins the invariant that matters and fails at 2048.

## [v1.0.9] — 2026-08-29

### Fixed
- **Epoch seal enforcement is on again, and a seal now binds only the workspace that made it.** The Hub built every repository with enforcement disabled. That was not a preference: ASG's epoch store was global, so completing a plan in one project sealed an epoch that hard-rejected ref updates in *every* project — switching the guard off was the only way to keep the Hub writable. agentstategraph v1.1.2 scopes an epoch to its owning workspace, so the opt-out is gone from all three construction sites and the guard finally does what it was always meant to.
- **The cross-workspace epoch view returned only the current workspace, and the workspace cards counted every workspace from the `default` listing.** `list_epochs` became workspace-scoped in agentstategraph v1.1.0; `/api/epochs?all=true` and `namespaces_summary` both still assumed it was global. The failure was partial rather than empty — legacy epochs read as `default` while newly sealed ones went missing — so it presented as "epochs stopped being sealed."

### Added
- **A startup migration stamps each legacy epoch with its owning workspace**, recovered from the `plan:<namespace>:<plan_id>` id that sealing writes. It is a migration and not an endpoint because an endpoint someone has to remember to call is not something the code reading that data can rely on. Assign-only underneath, so it cannot move an epoch between workspaces; ids carrying no namespace are skipped rather than guessed; a single unassignable epoch is logged and counted, never fatal. Verified against this hub's live database: 172 epochs assigned across 9 workspaces, 171 pre-namespace ids left unowned (every one of them already superseded by a namespaced twin), and the sealed commit sets byte-identical afterwards.

### Changed
- **A workspace's epoch count now comes from the epoch's `namespace` column rather than a match on its id prefix.** `epoch_id_prefix` survives in a reduced role — recovering a plan name from an id for display — and is no longer the source of truth for ownership.
- agentstategraph pin moves **v0.9.23 → v1.1.2**.

## [v1.0.8] — 2026-08-27

### Fixed
- **Recall savings were credited to the wrong workspace — every workspace card read zero.** Three independent faults compounded. (1) An MCP request naming no namespace and no session fell back to a shared `"default"` bucket, so every agent on a machine read and wrote one anonymous workspace and per-project memory was unreachable. (2) An explicitly named but *invalid* namespace was silently rewritten to `"default"`, so a typo leaked one project's memory into the shared bucket with no error. (3) A workspace only claims a session's tokens when its graph holds a node for that session, and session nodes were written solely by transcript import and `summarize_session` — both keyed on a per-conversation UUID that a live MCP session's id never matches, so live savings fell into the `default` complement bucket. Live MCP sessions now mint their own session node the first time they accrue savings, so the invariant holds by construction.
- **`ctx init --tool claude` also rewrote the global Claude Desktop config** with a single project's namespace, re-pointing every other project's agent. `--tool` now matches canonical tool slugs instead of substrings (`claude` → Claude Code; use `claude-desktop` for the other).
- **`ctx init --session <id>` was accepted and then silently ignored.** It now overrides the derived id.
- **Renaming or moving a repo orphaned its accrued savings.** The session id hashed the absolute path, so it changed with the directory. Identity now lives in a `.ctxproject.local` token at the worktree root and moves with the checkout.
- **One session row per workspace merged unrelated agents.** Session ids are now `<namespace>:<agent>:<checkout-token>`, so Claude, Codex and Gemini — and separate worktrees of one workspace — are each attributable.

### Added
- **`CTXONE_REQUIRE_IDENTITY`** (default **off**). When on, an MCP request that names no namespace or no session is refused with a 400 naming the remedy, instead of silently joining the shared workspace. An explicitly named *invalid* namespace is refused whether or not this is set — that path is the leak itself.
- **`?session=` on the MCP URL**, mirroring `?namespace=`. Clients that cannot send headers (Codex's `config.toml` accepts only a `url`) can carry a real identity instead of sharing the anonymous row.
- **`ctx init` locally excludes the configs it writes** (`.mcp.json`, `.gemini/`, `.cursor/`, `.vscode/`) via `.git/info/exclude`, never the tracked `.gitignore`. They embed a machine-local token, so committing one hands every clone the same identity.

## [v1.0.7] — 2026-08-25

### Fixed
- **Sealed checkpoints showed "0 commits" and a blank seal hash.** ASG's `EpochEntry.commit_count` reflects the empty create-time `commits` field — sealing stores the real set in `sealed_commits` and never updates the count — and the sqlite `epochs` table has no `seal_hash` column at all. The epoch view now reads the full epoch and reports `sealed_commits.len()` as "commits sealed"; the seal-hash column is removed from the API and both UIs rather than displaying a value that was never persisted. (The underlying engine gaps — persisting `seal_hash`, updating `commit_count` on seal, and making `export_epoch` bundles self-contained — are tracked against ASG.)

## [v1.0.6] — 2026-08-25

### Added
- **Sealed checkpoints are now visible and downloadable.** Sealing produced nothing but a count. `GET /api/epochs` lists a workspace's sealed epochs (`?all=true` for every workspace), and `GET /api/epochs/{id}/export` downloads one epoch's audit bundle as a JSON attachment. The workspace dashboard gains a **Sealed checkpoints** panel (plan · sealed date · commits) with per-row download, and a new `/epochs` route lists every workspace's checkpoints, linked from the sidebar under Activity › Checkpoints.

## [v1.0.5] — 2026-08-25

### Fixed
- **Completing a plan could brick the hub's write path.** ASG defaults epoch seal enforcement to *strict*: any ref update that would orphan a sealed commit is hard-rejected. Because the epoch store is global rather than per-namespace, one workspace's sealed per-plan epoch blocked writes in **every** workspace — observed live, with `plan_start` rejected by an unrelated workspace's epoch. CTXone seals per-plan epochs as viewable audit checkpoints, not immutability barriers, so the base repository is now built with `epoch_seal_strict(false)` and every forked namespace inherits it.

## [v1.0.4] — 2026-08-25

### Fixed
- Formatting-only release: `rustfmt` wrapping of the per-workspace epoch count filter, to satisfy the CI `fmt` gate. No behaviour change.

## [v1.0.3] — 2026-08-25

### Fixed
- **Every workspace card showed the same epoch count, and same-named plans collided.** ASG epochs live in one global table keyed by id with no namespace column, so the first cut reported the hub-wide total (171) on every workspace and let identically-named plans in different workspaces seal onto one shared epoch. Epoch ids are now `plan:<namespace>:<plan_id>`, and the Hub counts a workspace's own epochs by that prefix out of the global list — called once per summary, not once per namespace, to keep the rollup fast.

## [v1.0.2] — 2026-08-25

### Added
- **Completing a plan seals a checkpoint.** Every plan that reaches `completed` now yields a tamper-evident, exportable ASG epoch — a sealed snapshot of the workspace's memory graph at plan close — making the per-workspace epoch count meaningful with no user effort. Sealing is idempotent, fires on the active→completed transition from `plan_close` / `plan_complete` / `plan_done`, and runs on a background task so the reachability walk never blocks the tool call. `POST /api/plans/backfill_epochs` retroactively seals existing completed plans.

## [v1.0.1] — 2026-08-24

### Changed
- **Token headlines now lead with "new work" (input + output), not cache reads.** Agentic tools re-send the whole context on every tool step, so cache reads are routinely >95% of the raw count while costing about a tenth per token — a workspace read "5.5B tok" when the real work was ~200M. Cache reads are still shown, as a secondary figure with an input/output breakdown. Presentation only; the underlying data is unchanged.

### Removed
- The Team/Enterprise edition sections in the docs.

## [v0.9.47] — 2026-08-24

Tagged **after** `v1.0.0`, from four commits ahead of it — a numbering slip in a
multi-agent repo, not a release that precedes 1.0. Its contents reached the 1.0
line unchanged as `v1.0.1`; see that entry.

## [v1.0.0] — 2026-08-24

The 1.0 milestone. **No functional change from `v0.9.47`'s predecessor
`v0.9.46`** — this tag renumbers that tree, and the work it ships is described
under the `v0.9.37`–`v0.9.46` entries below rather than repeated here.

## [v0.9.46] — 2026-08-22

### Fixed
- **OpenAI/Codex cached tokens were double-counted.** OpenAI-family usage records count cached tokens *inside* `input_tokens`, whereas Anthropic reports input disjoint from cache reads — so summing the two double-counted the cached portion for OpenAI models (roughly 2× on cache-heavy sessions: SessionDrift 10.8B→5.5B) and overcharged cost, since cached tokens were billed at the input rate. The aggregation layer now subtracts the cached portion for OpenAI-family models, so "input" means fresh input for every provider and a turn's total matches the provider's own. Normalised in the recompute rather than at ingest, so stored turns keep their provider-native shape. Anthropic workspaces were already correct.

### Added
- CTXone Lens screenshots in the README (hub hero, plus workspace/code/sessions gallery).

## [v0.9.45] — 2026-08-21

### Fixed
- Two benign Lens 404s removed: the symbol call-graph probe and the turn-less session title probe.

## [v0.9.44] — 2026-08-20

### Fixed
- **Workspace cost was priced from a single representative model.** A workspace that used several models was costed as if all its tokens belonged to one of them. Each workspace tile is now priced per model from the per-model split.

## [v0.9.43] — 2026-08-20

### Fixed
- **`backfill_by_model` timed out on large databases.** The whole-namespace recompute ran unpaged; it is now paged so big graphs complete.

## [v0.9.42] — 2026-08-20

### Fixed
- **LLM usage inflated N× on re-ingest.** Ingest posts one aggregated `record_llm_usage` per run and the counter added to itself, so re-ingesting a session re-added its entire total — one session ingested 8× reported 6.5B input tokens against a true 817M, and the hub headline read 1.01T tokens. Displays now derive from the per-model split, which is recomputed from idempotently-upserted turns and had stayed accurate throughout.

## [v0.9.41] — 2026-08-19

### Fixed
- **Recall scanned the whole graph, including the transcript archive.** Topic matching ran an unindexed scan over every node value, dominated by `/sessions/**/turns` in a mature graph. Recall only ever surfaces memory, so it now reads the scoped subtree (default `/memory`) in a single pass. On a 2 GB copy of the live graph, recall is ~0.01s for common, rare and zero-match terms alike — previously 0.04–0.09s and degrading without bound as `/sessions` grew. Also a correctness win: recall can no longer surface transcript chatter.

## [v0.9.40] — 2026-08-19

### Added
- **Automatic recall, on a per-task gate.** `plan_new` / `plan_start` / `plan_next` now run a memory-scoped recall for the plan or task topic and return it inline as `recalled_memory`, so any agent — Claude, Codex, Cursor — gets the relevant prior decisions at task-lifecycle moments with no per-prompt or per-session hook. This restores an automatic trigger intended since 2026-08-04 but never wired up, which had left automatic recall with no trigger at all in the interim.
- **MCP session identity and persistence.** The MCP-over-HTTP path built each server with an ephemeral session counter that was never registered and never flushed, so recall savings from the gate — and from every MCP tool — vanished on disconnect. MCP services are now backed by the same shared, flushed session registry the REST hub uses, keyed by `X-CTXone-Session`, and `ctx init` writes that header for HTTP transports.

## [v0.9.39] — 2026-08-19

### Fixed
- **Workspace tiles read "0 tok ≈ $49520.76".** Each card printed session *context* tokens beside a cost priced from LLM *usage* records — unrelated measures, five orders of magnitude apart, and zero for 29 of the 30 workspaces on this hub. Tokens and cost are now shown on the same basis.
- Evicted `asd-serve` processes are killed on drop instead of lingering.
- Lens: an optional code panel that fails degrades in place instead of blanking the page, and an unreachable `asd-serve` reports why rather than guessing.
- **Releases no longer push `main` and the tag straight to GitHub** from a workstation — that bypassed the fail-closed leak-scan gate on the public mirror.

## [v0.9.38] — 2026-08-17

### Changed
- The Homebrew formula is rendered GitLab-side, dropping both tap secrets.

## [v0.9.37] — 2026-08-15

### Changed
- **GitHub Actions is the sole release publisher**; local cross-builds are retired. Two publishers racing on one release is how assets end up with `sha256`s that disagree with what CI built.
- Rolled agentstategraph v0.9.20 → v0.9.22 → v0.9.23.

## [v0.9.36] — 2026-08-13

### Fixed
- **Large ASD repos timed out on first load in the code proxy.** The pool's startup budget (10s) gated both binding *and* the first `/api/v1/health`, which warms the db with a symbol count over a possibly multi-GB store. A large repo (e.g. a 22GB iOS `.asd-state.db`) bound in ~1s but only passed health at ~10s, so it failed on first touch and only worked on a retry. The budget is now **45s**, tunable via `CTXONE_ASD_SPAWN_TIMEOUT_SECS`. (A pathologically bloated db whose health takes minutes still needs its store rebuilt — a longer wait won't save it.)

## [v0.9.35] — 2026-08-12

### Fixed
- **Intermittent "ASD server unreachable" on a cold repo.** The code-proxy process pool released its lock before spawning `asd-serve` and re-acquired it after, so concurrent first-touch requests each spawned a process; later stores overwrote and killed earlier children, and any request holding a killed process's URL failed. The Lens code page fires health + symbols + files + overview at once, so opening a cold repo reliably tripped this. Spawns are now single-flighted per repo (concurrent callers share one spawn), verified with 8 concurrent cold requests producing exactly one process.

## [v0.9.34] — 2026-08-12

### Fixed
- **ASD code proxy under launchd/systemd.** A Hub started by a service manager inherits a minimal `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin`), so the code-proxy process pool's bare `asd-serve` spawn failed with *"No such file or directory"* — every `/api/code/<repo>/*` call errored and the whole ASD integration was down, even though an interactive `ctx serve` (with a full PATH) worked and hid the bug. The Hub now resolves an absolute `asd-serve` path up front: explicit override (`--asd-serve-binary` / `CTXONE_ASD_SERVE_BINARY`) → `$PATH` → a sibling of the Hub's own binary (Homebrew installs them together) → common install dirs. The resolved path is logged at startup.

### Added
- **`--asd-serve-binary <PATH>` / `CTXONE_ASD_SERVE_BINARY`.** Pin the `asd-serve` binary the pool spawns, for setups where auto-resolution can't find it.

## [v0.9.33] — 2026-08-12

### Added
- **Opt-in session import onboarding.** The background sweep that pulled every transcript on the machine into the hub is now **off by default** — a fresh hub no longer silently ingests a machine's entire agent history. When enabled (`CTXONE_SESSION_SYNC_INTERVAL_SECS`), the sweep is **chunked per source** (claude/codex/cursor/gemini) with an env-tunable per-chunk timeout, so one large/slow source can't time out the whole run.
- **`ctx session list | import | ignore | unignore`.** Discover agent sessions on this machine as a stable, newest-first, **numbered** list (paged 25, with new/imported/ignored status), then import a chosen subset by number/range/`all` or via an interactive picker. Import is **key-aware** (prints a RICH-vs-PLAIN notice read from env/config — never prompts for or stores a secret) and routes each session to the workspace of the repo it ran in. `session ignore`/`unignore` maintain a **privacy skip-list** (`~/.ctxone/ignored-sessions.txt`) honoured by both manual imports and the background sweep. `session import --to <ns>` forces a workspace.
- **Graphical import in Lens.** The Sessions view gains an **Import sessions** panel: a checkable, filterable list of discovered transcripts with per-row and bulk **workspace assignment**, Import selected / Import all new, Mark private / Unmark, an **auto-sync toggle** (enable/disable the background sweep and its cadence at runtime — no restart, persisted across restarts), and a plain-language extraction-key note. New endpoints: `GET /api/sessions/discoverable`, `POST /api/sessions/import`, `POST /api/sessions/ignore|unignore`, `GET|POST /api/sessions/autosync`, and `GET /api/sessions/imported`.

### Fixed
- **`ctx doctor` no longer flags the canonical database as "stray."** The check compared raw file counts, so a `./ctxone.db` stub existing beside the real `~/.ctxone/memory.db` fingered *both* as strays. A stray is now specifically a stub that is not the canonical db; the message and suggestion name only the actual stragglers.
- **`why_did_we` returns rationale, not transcript chatter.** Session-transcript captures (full-turn / title / meta nodes under `/sessions/`) matched decision queries and buried the recorded rationale under an agent's own tool-call output. Both the HTTP and MCP paths now over-fetch and drop those paths.
- **Code-proxy 404 spam.** The Lens sidebar fired `/api/code/<repo>/health` + `/prefetch` for a repo that wasn't registered (a name persisted from another hub), 404-ing on every load; the calls are now gated on registry membership and a stale selection is cleared.
- **Live Tail stuck on "Connecting…".** A backgrounded tab skips polling, which left the feed on a permanent, misleading "Connecting…". The status is now driven by whether a poll has actually completed plus tab visibility: a hidden tab reads "paused (background)", an idle foreground feed reads "watching", and only a never-completed foreground poll reads "connecting".
- **`plan complete` vs `plan done` disambiguated in `--help`.** `plan done` now states it closes a single *task* (and points to `close`/`complete` for a whole plan); `plan complete` states it force-completes the whole *plan* (and points to `plan done` for one task).

### Changed
- **Cumulative token labels.** The workspace dashboard and per-session tiles relabel their token totals as **cumulative** ("Tokens used (cumulative)" / "tok · cumulative", with tooltips), so large running totals read as expected rather than as a single call.

## [v0.9.32] — 2026-08-11

### Added
- **`ctx ingest-session --dir-workspaces`.** Sessions imported from a working directory that isn't a git repo now route to a workspace named after the directory (kebab-cased basename) instead of the `default` namespace — grouping sessions by working directory the way Claude Code / Codex do. Opt-in: bare directory names are less canonical than a git remote (they can collide and proliferate), so it's off by default; git repos still key on their remote's `owner/repo`. This also makes a manually-moved non-git-dir session durable against a future re-sync (it routes back to its named workspace, not `default`).

## [v0.9.31] — 2026-08-11

### Added
- **Move a session to a workspace from the Lens Sessions view.** The selected session's detail header gains a `Workspace → [move to…] → Move` control that pins an imported session (one that landed in the wrong workspace, usually `default`) to the right one — a one-click UI over the guarded `move_session` endpoint. On success the list reloads and a "✓ Moved to `<ns>`" banner shows; errors render inline; the current workspace is excluded from the target list.

## [v0.9.30] — 2026-08-11

### Added
- **Safe cross-namespace move.** New `ctx move <path> --to-namespace <ns>` (with `--dry-run`, `--no-delete`, `--from-namespace`, `--to-branch`) and `POST /api/move` relocate an arbitrary subtree of graph paths to another workspace. Plus `ctx session move <id> --to <ns>` for CLI parity with `ctx plan relocate`. A new "Moving data between workspaces" section in the CLI reference documents all three and clarifies they are *not* the deny-by-default cross-namespace merge.
- **MCP namespace lookup tools.** `namespace_for_plan` and `namespace_for_task` scan every workspace to resolve which one owns a given plan/task id.
- **Lens plan scope selector.** The Plans view gains a scope control — **Plan** (one plan) → **All Plans** (every plan on the branch) → **All Branches** (every plan across the workspace) — with a filterable aggregate list (status, task counts, branch tag) and cross-branch open.
- **Glob search in Lens.** The plan switcher, task search, and aggregate list accept `*` (any run) and `?` (one char); a bare term stays a substring match.

### Fixed
- **Silent data-loss in cross-namespace moves.** `relocate_plan`/`move_session` deleted the source subtree *unconditionally* after a conditional write, so a corrupt/empty read (e.g. partial-tree corruption after many sequential deletes) or a skipped write could destroy the source. All moves now route through a single **write → verify (leaf-count) → delete** guard: the source is never deleted unless the target provably received the same leaves.
- **MCP fail-closed on an underivable namespace.** The stdio MCP server silently fell back to the `default` namespace when it couldn't derive a workspace from the cwd, mis-filing writes. Write tools now refuse with actionable guidance (reads still work; an explicit `default` is honored), and read tools annotate results with a workspace notice on a fallback-`default` session.
- **Lens Plans loading state.** Switching workspaces briefly rendered "No plans on this branch yet" during the in-flight reload; it now shows a loading placeholder.

## [v0.9.29] — 2026-08-10

### Added
- **Browse-with-search on Search, Recall, and Why.** The Lens Browse tree+detail panel is now embedded beside the results on the Search, Recall, and Why pages: clicking a result (or a Why trace path) reveals it in the in-page tree and loads its value/provenance without navigating away to `/browse`. The browser is extracted into a reusable `BrowsePane` component; `/browse` itself keeps its `?path=` deep link.

### Fixed
- `BrowsePane` no longer flashes "No memory on this branch yet" during its initial path load. The empty state was shown whenever the path list was empty — including the in-flight first fetch, which is slow because it returns the whole tree — so a cold load briefly and misleadingly claimed the branch had no memory. It now shows a "Loading memory…" placeholder until the first load completes.

## [v0.9.28] — 2026-08-10

### Changed
- Split the Lens "Token economics" panel into distinct **Recall savings** and **LLM usage** views, and broke **Model efficiency** out into its own panel.

### Fixed
- Corrected inflated cost figures: `$/1M` now excludes cache reads, and the recall savings estimate no longer double-counts.

## [v0.9.27] — 2026-08-05

### Changed
- CLI output no longer frames the per-recall flat-vs-injected ratio as "savings" — the last place still using the old "N tokens sent vs M flat (X× savings)" language. `ctx recall` now reports just the honest injected-token count, and `ctx demo`'s cumulative line reads "Estimated savings this session (conservative model … can't be measured) — N injected · ~M saved (est.)", consistent with the Lens reframing in v0.9.26.

## [v0.9.26] — 2026-08-05

### Changed
- Token savings is now presented as an honest, bounded **estimate** rather than a measurement. The old figure divided the entire serialized memory graph by each recall injection and accumulated it per call, producing implausible ~9000× ratios and billions of "saved" tokens. Savings from a memory tool is inherently a counterfactual (the avoided run never happened), so `tokens_saved` is now a conservative model — `(RECONSTRUCTION_FACTOR − 1) ×` the real injected recall payload — that grows with the session and never touches the graph. Lens tiles are relabeled "Tokens saved (est.)" and the flat-memory ratio is removed.
- Session detail leads with the efficiency (burn) verdict instead of the discredited savings ratio; a burning/diminishing session now offers a **"Roll to a new session"** action.

### Added
- `GET /api/sessions/{sid}/starter` — generate a fresh-session seed from the user's own words (verbatim `user_text`, de-noised, grouped by topic arc). Pure, no-LLM. Surfaced in Lens as the payload the "roll to a new session" CTA copies.
- `SessionSnapshot.session_startup_tokens` — the first-recall payload, surfaced as the session's startup boost.

## [v0.9.25] — 2026-08-05

### Added
- Depth-capped shallow state reads (`t-007`); agentstategraph bumped to v0.9.18.
- `session reattribute` enumerates the graph and resolves the working directory from it, rather than trusting the caller.

### Fixed
- Recall dedupes redundant searches and caches results keyed by graph head.

## [v0.9.24] — 2026-08-04

### Added
- **Session analysis pipeline (P1–P8).** A pluggable extraction provider, hub-stored analysis status, cost estimation with a price table, session summaries with topic arcs (pseudo-branches), a generic import source for non-local clients, `ctx analyze list | estimate`, a Lens analysis panel on the session detail view, and user-facing toggles to control it.
- Watermarked memory extraction for the hub's background sweep, so a re-run resumes rather than reprocessing.

## [v0.9.23] — 2026-08-04

### Added
- **Background session sweep** — capture is pulled on a schedule rather than only on demand.

### Fixed
- The installed hub service bakes in `--ctx-binary`, so it resolves under a service manager.

### Removed
- The per-prompt recall hook in project settings. (Recall was intended to move to a per-task gate at this point; that gate was not actually implemented until v0.9.40, leaving the interim with no automatic trigger.)

## [v0.9.22] — 2026-08-03

### Added
- **Summary-gated plan close** — closing a plan requires a summary; the engine no longer auto-completes plans.
- New worktrees auto-enable `.githooks`, and a pre-push `rustfmt` gate stops formatting failures reaching CI.

## [v0.9.21] — 2026-08-03

### Fixed
- Formatting-only release: `rustfmt` on `source_ref_for_path` and its test. No behaviour change.

## [v0.9.20] — 2026-08-03

### Added
- `ctx branch reset <name> --to <ref> [--backup]` and `POST /api/branches/reset` — reset a branch to a ref, optionally snapshotting a timestamped recovery ref first.
- `ctx merge --dry-run` / `--allow-deletions` across CLI, HTTP, and MCP — preview a merge and gate merges that would delete nodes.
- `ctx init --no-mcp` — skip the MCP config step during init; AGENTS.md is primed non-interactively (no Y/n prompt).

### Changed
- Merge now enforces a plans-domain policy: rejects done→pending task regressions, promotes active→completed plans post-merge, and preserves task proofs.

### Fixed
- `archive_plan` resolves the branch from the query string then the JSON body, fixing spurious 404s when archiving plans on non-default branches.

## [v0.9.19] — 2026-07-29

### Added
- `ctx db upgrade [--check]` — run pending schema migrations behind a snapshot and a post-upgrade `fsck` integrity check.

### Changed
- Plan/task/doc read helpers (`plan next` in-progress, `plan stale`, `docs find`) now surface repository-integrity errors instead of masking a corrupt tree as an empty result.
- CHANGELOG version headers no longer carry a `v` prefix, matching AgentStateGraph and AgentStateDeveloper.

## [v0.9.18] — 2026-07-28

Cut a few hours after `v0.9.17` the same evening, on top of it.

### Added
- **`ctx db fsck [--repair]`** — an integrity check that walks refs → commits → state roots → objects, reports missing or dangling objects and the nearest readable ancestor, and can rewind a damaged ref. Paired with surfacing tree corruption instead of masking it: a dangling object used to be reported as an empty result, so corruption looked like "no data".

### Fixed
- Slash-containing branch names are percent-encoded, so refs like `codex/drift-pad-navigation` resolve instead of 404ing.

## [v0.9.17] — 2026-07-28

### Changed
- **The public GitHub mirror is now a gated `publish-github` job**, replacing push-mirroring, so only scanned branches and tags reach it — push-mirroring exposed every branch. Private GitLab dependencies (including the `@agentstate/lens-core` npm package) moved to public GitHub, and dead GitLab token dep-auth was scrubbed from CI.
- `install.sh` rewritten to the tarball pattern.
- Bumped agentstategraph v0.9.4 → v0.9.6.

### Fixed
- Lens auto-selects the first plan on a workspace or branch switch instead of erroring.
- Test fixtures use generic example paths, keeping leak-scan clean.

## [v0.9.16] — 2026-07-21

A large release centred on **workspace-scoped sessions**, **more import
sources**, **ingest performance**, and a **session-efficiency ("burn") board**
in Lens. Summarised from the commit history.

### Added — sessions & workspaces

- Each transcript is routed to its own **workspace**; existing sessions are
  migrated out of `default` into their workspaces, and the session list is
  scoped accordingly.
- Sessions can be deleted and stay deleted.
- Per-turn **git provenance** captured and rolled up per session.
- Plan/task links derived server-side; `--namespace` honoured explicitly.
- A session's **burn score** is persisted at ingest and read by the dashboard.

### Added — import sources

- **Gemini and Cursor** session ingestion, extending the `SessionSource` seam.
- `--since` skips whole old sessions; `--scan-dir` imports from custom dirs.

### Added — Lens

- A **burn board** ranking the least-efficient sessions: scans every session,
  ranks the worst, caches per workspace+branch, with a time-range column and a
  scrollable list.
- A "Recently updated" dashboard panel.
- Independent scrolling for the session list and detail panes; the app shell is
  pinned so the document no longer scrolls.

### Changed — performance

- SQLite opened in **WAL mode** (`synchronous=NORMAL`); `agentstategraph` bumped
  to v0.9.4. ~1.75× faster cold import.
- Ingest: concurrent sessions, bulk turn writes with aggregated token records,
  index-only Cursor discovery, and skipping unchanged sessions on re-sync.

### Fixed

- `--file` always imports, ignoring a remembered `--since` window.
- Request body limit raised so large bulk turn writes don't 413.
- The web unit suite is runnable again and runs in CI; `npm ci` unbroken.

## [v0.9.15] — 2026-07-19

Two themes: the first step of **multi-agent session import** (Claude Code is no
longer the only source), and a round of **Lens fixes** for figures that were
quietly misleading.

### Added — multi-source ingestion

- A `SessionSource` seam splits "where transcripts live" from "what we do with
  them". Claude Code moves behind it unchanged; `metrics.rs` no longer carries a
  duplicate copy of the directory walk.
- **Codex adapter** — imports `rollout-*.jsonl` from `~/.codex/sessions` and
  `~/.codex/archived_sessions`. Verified against 195 sessions / 5,865 turns.
- `ctx ingest-session --source claude|codex|all`. An unknown source is an error
  listing the known ids, not a silent empty result.
- Session ids are namespaced by source (`codex:<uuid>`). Claude Code is exempt:
  its ids are already keyed into existing rows, memory tags and turn paths.
- Sessions now carry the real agent label in `source` instead of a hardcoded
  "Claude Code", and the reported provider is derived from the model rather than
  always being `anthropic`.

### Added — source-native token classes

- `extra_tokens` preserves token classes the four normalised counters cannot
  express — Codex `reasoning_output_tokens`, Gemini `thoughts`/`tool` — verbatim
  under the reporting agent's own field names, persisted in a new sparse column.
  Folding them into `output_tokens` would have destroyed the distinction and made
  cross-agent comparison wrong.
- `GET /api/stats/activity/{ref}?days=N` — per-day commit counts for the
  activity heatmap, aggregated server-side.

### Fixed

- **Session stats were lost on SIGTERM.** The shutdown handler awaited `ctrl_c`
  alone despite its comment; `launchctl bootout`, `docker stop`, a reboot and a
  plain `kill` all skipped the final flush, dropping up to 30s of usage on every
  service stop.
- **Auto-refresh blanked the page.** Pages guarded on a bare `loading` flag
  replaced their content with a skeleton every poll, losing scroll position and
  any open panel. Refresh cadence also widened 15s → 30s.
- **The activity heatmap charted a commit-count window, not a time window.** It
  bucketed the 1000 most recent commits, so a busy machine saw *less* history —
  at one point 80 minutes. Now a fixed, zero-filled day window.

### Changed — Lens

- Session commits are colour-coded by conventional-commit type with an
  orphan/linked accent, plus type and linkage filters.
- New **token usage over time** charts, total and split by model.
- LLM consumption figures render compact (5.6B) with the exact value on hover.
- **Every dashboard panel now states its scope** — `all branches` for global
  token figures, the branch name for branch-scoped ones. The dashboard mixes
  both, and an empty Activity panel previously read as "collection stopped"
  rather than "this branch finished in May".

### Known gaps

- Cursor, Gemini CLI and Android Studio adapters are designed but not built.
- Memory-op savings still pool into the `default` session: MCP-over-HTTP does not
  resolve a session id, so per-session `tokens used`/`saved` read 0 for sessions
  created since HTTP became the default transport.
- The heatmap intensity ramp is linear against the maximum, so one very busy day
  flattens the rest of the range.

## [v0.9.14] — 2026-07-18

The headline work since v0.9.13: project **namespaces** and the **unified
MCP-over-HTTP daemon** that makes one process serve MCP, the REST API, and the
Lens web UI.

### Added — namespaces & projects

- Project **namespaces** isolate everything ref-scoped (branches, plans, memory,
  taints, reminders, history); one namespace typically maps to one code repo.
- `ctx project add/list/use/detect` plus a `POST /api/projects` registry;
  namespace threaded through every ref-touching endpoint (query param,
  `X-CTXone-Namespace` header, else `default`).
- Per-session namespace resolution in MCP + a `project_status` tool.
- Git-branch mirroring **within** a project namespace.
- `ctx plan list --all-namespaces` for a global plan inventory.

### Added — unified MCP-over-HTTP daemon (now the standard setup)

- **MCP over HTTP** (Streamable HTTP) at `POST /mcp?namespace=<ns>`: one
  `ctxone-hub --http` process serves MCP + REST + Lens. Removes the
  stdio startup-order / db-lock race.
- `ctx init` now defaults to `--transport http`, writing URL-based configs
  (native `http` for Claude Code/Cursor/VS Code/Codex; an `mcp-remote` stdio
  bridge for Claude Desktop). Preflights the daemon; stdio remains a fallback.
- `ctx service install/uninstall/status` — run the daemon as an always-on
  login/boot service (launchd, systemd, or Windows Task Scheduler).

### Added — security posture

- Optional **bearer-token auth** over REST + `/mcp` (`--auth-token` /
  `CTXONE_AUTH_TOKEN`): non-loopback requests need `Authorization: Bearer`;
  loopback is exempt. Tokens are injected into `ctx init --transport http` configs.
- **Origin guard + tightened CORS** (CSRF / DNS-rebinding): same-origin always
  passes; foreign origins rejected unless allow-listed (`--allowed-origin` /
  `CTXONE_ALLOWED_ORIGINS`). Lens is local/tunnel-only over an authed hub.

### Added — plans, data & docs

- `ctx plan link` — a task can satisfy a task in another plan (cross-plan);
  `ctx plan stale` — surface in-progress tasks idle for N days;
  `ctx plan next --in-order` + in-progress tasks shown separately;
  non-blocking warning when starting a task while another is in progress;
  richer proof-parse errors with per-kind examples.
- `ctx db export` / `ctx db import` — portable JSON branch-graph snapshots.
- `ctx docs` registry — index canonical docs; `import-doc` alias for `prime`.
- `ctx reminder tick` — executor for due reminders: runs a reminder only if it
  is explicitly **`autonomous: true`** **and** every one of its commands is on an
  exact-match allowlist (`~/.ctxone/reminder-tick.allow`); records the outcome
  and snoozes anything unapproved. (Gating on the `autonomous` flag directly,
  not just status, is fail-closed and sidesteps an `agentstategraph-reminders`
  bug where `remind_me` can promote a non-autonomous reminder straight to `due`.)
- `ctx service tick install/uninstall/status` — install the tick on a periodic
  timer (launchd `StartInterval`, a systemd `.timer` + oneshot `.service`, or a
  Task Scheduler repetition trigger). `--interval`, `--allowlist`, `--skip`.
- New MCP tools `plan_link`, `plan_stale`, `docs_find`.

### Added — Lens

- Namespace switcher + `/projects` page; reminders, recall, why-did-we, and
  live-tail pages; `/code/*` renders from `@agentstate/lens-core`; streaming
  (SSE) passthrough for the ASD proxy.

### Changed

- Agent-discoverability polish across the CLI help, MCP tool descriptions, and
  the reference docs (`AGENTS.md` task-discipline + docs-model rules).
- **Reminders now fail closed**: the `autonomous` flag defaults to `false`
  (was `true`). A reminder created without an explicit `autonomous: true`
  surfaces as `awaiting_permission` and must be `reminder_approve`d before any
  executor acts on it — safer now that reminders can carry commands an executor
  will run. Set `autonomous: true` explicitly for unattended reminders.

## v0.9.13 — 2026-06-08

### Fixed

- `release.yml` builds the SvelteKit frontend before `cargo` so Lens assets
  ship in the release binary.

## v0.9.12 — 2026-06-08

### Added

- GitHub Actions release pipeline (first automated release) + Windows
  distribution; `scripts/release.sh` + `RELEASE.md`.
- Lens surfaces Plan G/K thinking from `asd-serve`; sidebar split into CtxOne
  and ASD sections.

## v0.9.11 — 2026-06-03

### Added — asd-multi-repo integration

- Shared registry at `~/.config/asd/repos.toml` consumed by the `asd` CLI,
  `asd-mcp`, and CTXone Hub. Auto-discovered by the hub at startup when no
  `--asd-url` / `--asd-path` flags are given.
- `AsdProcessPool` in the hub: lazy-spawns `asd-serve` per registered repo,
  evicts idle children. Flags `--asd-path name=/path/db` and
  `--asd-idle-timeout <secs>`.
- MCP tools `set_active_repo(repo)` and `get_active_repo()` with
  per-session active-repo resolution: explicit > session > single registered.
- Lens repo picker now shows hot/cold status dots and fires a prefetch on
  selection.
- New `POST /api/code/{repo}/prefetch` endpoint warms a pool entry.
- New `/code/thinking` page surfaces Plan G/K thinking captured by asd
  (hypotheses, mental models, open questions, failed attempts), with a
  confidence-floor slider. Symbol detail page grows an "Inherited thinking"
  panel.
- End-to-end harness at `scripts/e2e/asd-multi-repo.sh`.

### Changed — Homebrew tap migration

The Homebrew tap moved from `ctxone/tap` (GitHub `ctxone/homebrew-tap`,
now archived) to `agentstatelabs/ctxone` (GitHub
`agentstatelabs/homebrew-ctxone`, mirrored from
`git.internal.example/agentstategroup/homebrew-ctxone`). Release
tarballs likewise moved from `ctxone/ctxone-docs/releases` to
`agentstatelabs/ctxone-releases/releases`.

Existing users on the old tap should run:

```sh
brew untap ctxone/tap        # untap first if you've already migrated install source
brew tap agentstatelabs/ctxone
brew reinstall ctxone        # picks up the new release URL
```

(`brew untap` refuses while a formula from that tap is still installed —
do the `reinstall agentstatelabs/ctxone/ctxone` first if needed.)

### Changed — UI

- Lens sidebar split into "CtxOne" and "ASD" sections. Repo picker moves
  under ASD; branch picker stays under CtxOne.
- Plans page header reorganised into two rows.
- Dev workflow: `npm run dev` proxies `/api/*` to a locally-running hub
  (override target via `VITE_HUB_URL`).

### Fixed

- Empty-repo race on first render of `/code` — `selectedRepo` now
  hydrates synchronously from localStorage at module import.
- `AsdProcessPool` correctly parses the resolved bind address from
  `asd-serve` stdout (asd-serve 1.0.81+) and gates on `/api/v1/health`.

## v0.9.10 — 2026-05-13

- Rebased versioning to the 0.9.x series after the 0.8x experiment.
- See commit `274f679` for the full diff.

## Prior history

For releases before v0.9.10 see `git log` and the archived GitHub
repos under `ctxone/` (now archived; canonical history continues on
`git.internal.example/agentstategroup/ctxone`).

## [v0.9.13] — 2026-06-08

### Fixed
- The release workflow builds the SvelteKit frontend before cargo, so `rust-embed` finds `web/build/`.

## [v0.9.12] — 2026-06-08

### Added
- **GitHub Actions release pipeline plus Windows distribution**, `scripts/release.sh` and `RELEASE.md` — the first automated release.
- Lens surfaces Plan G/K thinking from `asd-serve`; the sidebar is split into CtxOne and ASD sections.
- This CHANGELOG.

## [v0.9.11] — 2026-06-02

The consolidation release that renumbered the project onto the `0.9.x` line and
moved it to GitLab. It carries roughly two months of work from the previous
`0.7x`/`0.8x` line:

### Added
- **Multi-repo code intelligence (M18/M19)** — an ASD process pool with configurable idle timeout and health gating, a Code Lens view layer with a repo picker and hub routing table, code-intelligence MCP tools, `set_active_repo` / `get_active_repo` with a session-scoped active repo, auto-discovery of ASD repos from `~/.config/asd/repos.toml`, hot/cold status and a prefetch endpoint, and `--asd-path` / `--asd-idle-timeout`.
- **`ctx session metrics`** with cost estimation and unit-of-work detection, per-turn model pricing, and an explicit cache discount in every metrics view.
- Reminders across the stack.

### Changed
- The agentstategraph engine moved from a git submodule to a pinned git dependency; rolled 0.8.1 → 0.9.2.
- Lens builds as a pure client-side SPA so dynamic routes build cleanly; CI builds the frontend before cargo.
- Moved to GitLab CI with a pinned toolchain; clippy gates narrowed to correctness.


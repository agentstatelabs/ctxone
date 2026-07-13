# CLI Reference

Complete reference for the `ctx` command-line tool. For task-oriented
recipes, see [COOKBOOK.md](COOKBOOK.md). For the mental model, see
[ARCHITECTURE.md](ARCHITECTURE.md).

## Synopsis

```
ctx [GLOBAL OPTIONS] <COMMAND> [COMMAND OPTIONS]
```

## Global options

Every command accepts these flags. They can also be set via environment
variables.

| Flag | Env var | Default | Description |
|------|---------|---------|-------------|
| `--server <URL>` | `CTX_SERVER` | `http://localhost:3001` | Hub HTTP endpoint |
| `--branch <REF>` | `CTX_BRANCH` | see below | Branch / ref to read and write |
| `--namespace <NS>` | `CTX_NAMESPACE` | auto-detect | Project namespace to operate in |
| `--format <FMT>` | `CTX_FORMAT` | `text` | Output format: `text`, `json`, or `id` |
| `--help` / `-h` | | | Print help |
| `--version` / `-V` | | | Print version |

**Namespace resolution.** When `--namespace` / `CTX_NAMESPACE` is
absent, every Hub-bound command resolves the namespace via the project
detection chain for the current directory: a `.ctxproject` file in the
cwd or any parent, then the repo's git remote looked up in the Hub's
project registry. No match — or Hub unreachable — silently falls back
to the `default` namespace. See [`ctx project`](#ctx-project--map-repos-to-namespaces)
below.

**Branch resolution.** Precedence for the working branch:

1. `--branch` / `CTX_BRANCH` (explicit always wins)
2. **Git mirror** — inside a project namespace, the sanitized current
   git branch (`feature/x` → `feature-x`), auto-created from `main` on
   first use. Detached HEAD → no mirroring.
3. Config-file `branch` (`~/.ctxone/config.toml`) — applies outside
   projects only
4. `main`

## Output formats

- **`text`** — human-readable (default). Pretty-printed, with headings and
  summary lines.
- **`json`** — pretty JSON of the full response. Designed for `jq`, `grep`,
  and other tool-chain use.
- **`id`** — minimal: just the canonical identifier (name / commit_id /
  path). For capture into shell variables: `fact=$(ctx remember "..." --format id)`.

## Exit codes

Follows `sysexits.h` conventions.

| Code | Name | Meaning |
|------|------|---------|
| 0 | OK | Success |
| 64 | USAGE | Clap handled an argument error |
| 65 | DATAERR | Bad input data (empty fact, malformed file, no sections) |
| 66 | NOINPUT | Input file doesn't exist or can't be read |
| 69 | UNAVAILABLE | Hub unreachable at `--server` |
| 70 | SOFTWARE | Internal error; `ctx doctor` failed a check |
| 74 | IOERR | Failed to read from stdin or write to a file |
| 76 | PROTOCOL | Hub returned a 5xx or an unexpected response |

---

## Memory commands

### `ctx remember <fact>`

Store a fact, preference, or decision.

```
USAGE: ctx remember <FACT> [OPTIONS]

ARGS:
  <FACT>  The fact to remember. Use "-" to read from stdin.

OPTIONS:
  -i, --importance <LEVEL>  high | medium | low  [default: medium]
  -c, --context <NAME>      Group under /memory/<context>/
  -t, --tags <TAGS>         Queryable tags
```

**Examples:**

```bash
ctx remember "We use BSL-1.1" --importance high --context licensing
echo "fact" | ctx remember -
ctx remember "deployed" --format id    # prints just the commit id
```

### `ctx recall <topic>`

Retrieve memories relevant to a topic. Always includes pinned context first,
then topic-matched facts, respecting a token budget.

```
USAGE: ctx recall <TOPIC> [OPTIONS]

ARGS:
  <TOPIC>  Topic to search for (single or multi-word; tokenized)

OPTIONS:
  -b, --budget <N>  Max token budget for the response  [default: 1500]
      --exact       Re-tokenize response + full graph locally with
                    tiktoken (cl100k_base) and show exact counts
                    alongside the fast 4-char estimate
```

### `ctx tokens [text]`

Count exact tokens in a piece of text using tiktoken's cl100k_base
encoding (GPT-3.5 / GPT-4 family). Reads from stdin if no argument
or `-` is given. Shows both the exact count and the 4-char estimate
for comparison.

```bash
ctx tokens "The quick brown fox jumps over the lazy dog"
# 43 chars
# 9 tokens (cl100k_base, exact)
# 10 tokens (4-char estimate)

echo "any text" | ctx tokens -
```

### `ctx prime <file>`

Load a markdown file as structured memory, split at H1 and H2 headings.
Idempotent by `--source` name (re-running overwrites).

```
USAGE: ctx prime <FILE> [OPTIONS]

ARGS:
  <FILE>  Path to markdown file, or "-" for stdin

OPTIONS:
      --pin              Always include these sections in every recall
      --source <NAME>    Source name (default: file stem)
```

### `ctx pinned`

List all pinned memories, grouped by source.

### `ctx forget <path>`

Delete a memory at an exact path.

```
USAGE: ctx forget <PATH> [OPTIONS]

ARGS:
  <PATH>  Path to forget (from ctx search or ctx ls)

OPTIONS:
      --reason <TEXT>  Shows up in blame  [default: "forgotten by user"]
```

### `ctx context <project>`

Load the full context tree for a project (everything under
`/memory/projects/<project>/`).

---

## Graph visibility commands

### `ctx search <query>`

Literal substring search across all values. Unlike `recall`, this is not
LLM-oriented: no token budget, no pinned-first behavior, all results returned.

```
USAGE: ctx search <QUERY> [OPTIONS]

OPTIONS:
  -n, --max <N>  Max results  [default: 50]
```

### `ctx ls [prefix]`

List paths in the graph under a prefix.

```
USAGE: ctx ls [PREFIX] [OPTIONS]

ARGS:
  [PREFIX]  Prefix to list under  [default: /]

OPTIONS:
      --max-depth <N>  Max tree depth  [default: 50]
```

### `ctx get <path>`

Read a value at an exact path. Pretty-printed as JSON.

### `ctx log [options]`

Recent commit history.

```
USAGE: ctx log [OPTIONS]

OPTIONS:
  -n, --limit <N>  Max commits to show  [default: 20]
```

### `ctx blame <path>`

Show the provenance chain for a path — who wrote it, when, and why.

### `ctx diff <ref_a> <ref_b>`

Compare two refs (branches, tags, or commits).

```
USAGE: ctx diff <REF_A> <REF_B>

OUTPUT:
  +  AddKey       /memory/test/abc
  ~  SetValue     /memory/foo/bar
  -  RemoveKey    /memory/baz
```

### `ctx tail [--interval MS]`

Tail -f style live monitor of new commits. Polls the log endpoint at the
given interval (default 2000ms). Ctrl-C to stop.

---

## Branch commands

### `ctx branches`

List all branches. The current branch (from `--branch` / `CTX_BRANCH`) is
marked with `*`.

### `ctx branch <name>`

Create a new branch.

```
USAGE: ctx branch <NAME> [OPTIONS]

OPTIONS:
      --from <REF>  Ref to branch from  [default: main]
```

### `ctx merge <source>`

Merge `source` into a target branch (default `main`). Conflicts come back
as a structured 409 — exit code `EX_DATAERR` with the conflicting paths
listed on stderr. On success, prints the new commit id.

```
USAGE: ctx merge <SOURCE> [OPTIONS]

OPTIONS:
      --into <REF>      Target branch  [default: main]
      --message <TEXT>  Commit description  [default: "Merge"]
```

Merging across namespaces is denied by default (the Hub returns 403).

---

## `ctx project` — map repos to namespaces

A **project** maps a code repo to its own namespace holding that repo's
branches, plans, memory, taints, reminders, and history. Detection is
automatic per-command (`.ctxproject` file, then git remote); run
`ctx project add` once per repo to opt in. Repos without a project —
and all pre-existing data — live in the reserved `default` namespace.

Inside a project namespace, **branch mirroring** kicks in: the working
branch defaults to the sanitized current git branch (see
[Global options](#global-options)), so memory written on
`feature/x` lands on the ASG branch `feature-x` without any flags.

### `ctx project add <id>`

Register the current repo as a project. Creates the namespace on the
Hub (the id doubles as the namespace name), binds the local path,
records the git remote for detection, and writes a `.ctxproject`
marker (one line: the project id) at the repo root. Commit the marker
so agents auto-detect the project in every checkout.

```
USAGE: ctx project add <ID> [OPTIONS]

ARGS:
  <ID>  Project id (kebab-case). Doubles as the namespace name.

OPTIONS:
      --display-name <NAME>  Human-readable name (defaults to the id)
      --path <PATH>          Repo root to bind (default: git root of cwd, else cwd)
      --no-marker            Skip writing the .ctxproject marker file
```

### `ctx project list`

List registered projects with their namespaces.

### `ctx project use <id>`

Point this checkout at an existing project: binds the path on the Hub
and writes `.ctxproject` at the repo root (skip with `--no-marker`).
Use it for a second clone or worktree of an already-registered repo.

### `ctx project detect`

Show which project/namespace the current directory resolves to, and
via which mechanism (`.ctxproject` file or git remote).

---

## Provenance & sessions

### `ctx why-did-we <decision>`

Trace a decision back through `recall` matches to the commits that wrote
each value, including the agent, intent, and timestamp. Mirrors the
`why_did_we` MCP tool — useful for "who decided this and when?".

### `ctx summarize-session -p "<point>" [-d "<decision>"]`

Capture key points and decisions for the current session. Stored under
`/sessions/<id>/{summary,decisions}`. Session id resolves from the
global `--session` flag or `CTX_SESSION` env (errors if neither is set).
Mirrors the `summarize_session` MCP tool.

```
USAGE: ctx --session <ID> summarize-session [OPTIONS]

OPTIONS:
  -p, --point <TEXT>     One bullet (repeatable, required)
  -d, --decision <TEXT>  One decision (repeatable)
```

### `ctx record-usage --input N --output N [OPTIONS]`

Report LLM token usage for the current session — accumulates per-session
counters in the Hub. Mirrors the `record_llm_usage` MCP tool.

```
OPTIONS:
      --input <N>          Prompt tokens consumed
      --output <N>         Completion tokens generated
      --cache-read <N>     Cached prompt tokens read   [default: 0]
      --cache-create <N>   Cached prompt tokens written [default: 0]
      --model <NAME>       Model identifier (e.g., claude-sonnet-4-5)
      --provider <NAME>    Provider name
```

---

## Taint commands

Guardrails for sensitive paths — `taint` (warn/block on write),
`quarantine` (require authorization), `watch` (audit only). Mirrors the
`taint_*` MCP tools.

### `ctx taint list [--path-prefix <P>] [--kind <K>] [--include-resolved]`

List active taints. Filter by path prefix, by `taint`/`quarantine`/`watch`,
or include resolved entries.

### `ctx taint check --path <P> [--confidence <F>]`

Ask whether a write to `path` would be allowed at a given confidence
(default 1.0). Returns `can_write`, the strongest blocking effect, and
the matching taint id.

### `ctx taint apply --path <P> --name <N> --kind <K> --reason "<text>" [OPTIONS]`

Apply a taint, quarantine, or watch.

```
OPTIONS:
      --effect <EFFECT>     warn | block | review | isolate | advisory (taint only)
      --severity <SEV>      low | medium | high | critical  [default: medium]
      --authorized <AGENT>  Agent allowed through quarantine (repeatable)
```

### `ctx taint remove --taint-id <ID> --reason "<text>"`

Resolve an existing taint.

---

## Operations commands

### `ctx status`

One-line Hub health check plus session token summary. Also shows the
active project/namespace (and how it was detected) for the current
directory.

### `ctx stats`

Detailed token savings breakdown.

```
CTXone Token Savings
  graph size:   451 tokens
  tokens sent:  98
  tokens saved: 1706
  savings:      18.4x
```

### `ctx demo`

Seed 21 realistic facts and run 4 recalls, showing per-query and cumulative
savings. Use for first-time demos or to verify a fresh install.

### `ctx doctor`

End-to-end health check. Verifies:

- `ctxone-hub` binary is discoverable
- `~/.ctxone/memory.db` parent is writable
- Hub HTTP endpoint is reachable
- `main` branch is accessible
- Each detected AI tool has a CTXone MCP config

Prints each check with ✓ or ✗, plus suggested fixes. Exits 70 on failure so
scripts can gate on it.

### `ctx serve [options]`

Start the Hub. Delegates to the `ctxone-hub` binary.

```
USAGE: ctx serve [OPTIONS]

OPTIONS:
  -p, --port <PORT>             Port  [default: 3001]
      --storage <TYPE>          sqlite | postgres | memory  [default: sqlite]
      --path <PATH>             Database path  [default: ~/.ctxone/memory.db]
      --http                    Also start HTTP API (otherwise stdio MCP only)
      --lens                    Serve the Lens web UI at / (requires --http)
```

For the Postgres backend, the connection string is read from the
`DATABASE_URL` environment variable, not a flag:
`export DATABASE_URL=postgres://… && ctx serve --http --storage postgres`.

**ASD code intelligence.** The ASD code-pool flags `--asd-path` and
`--asd-url` are **not** on `ctx serve` — they're passthrough flags on the
`ctxone-hub` binary directly. (In practice you rarely need them: with no ASD
flags the hub auto-discovers repos from `~/.config/asd/repos.toml`.)

```bash
# Hub manages asd-serve processes — recommended for most setups
ctxone-hub --http \
  --asd-path myproject=/home/user/myproject/.asd-state.db \
  --asd-path otherlib=/home/user/otherlib/.asd-state.db

# Pre-running asd-serve (useful when you want full control over the process)
asd-serve --db /home/user/myproject/.asd-state.db &
ctxone-hub --http --asd-url myproject=http://127.0.0.1:4120
```

- `--asd-path <NAME=PATH>` — register an ASD repo db for the code-intelligence
  process pool (repeatable). The hub spawns `asd-serve` on demand and kills
  it after 5 min idle. (`--asd-repo` is accepted as a legacy alias.)
- `--asd-url <NAME=URL>` — register a pre-running `asd-serve` endpoint
  (repeatable). Proxies `/api/code/<name>/*` to `<URL>/api/v1/*`.

See [ASD_INTEGRATION.md](ASD_INTEGRATION.md) for the full setup guide.

**Auth & origin (network-exposed hubs).** `ctxone-hub` accepts a bearer token
and an origin allow-list, guarding both REST and `/mcp`; loopback is exempt:

```bash
ctxone-hub --http --auth-token "$CTXONE_AUTH_TOKEN" \
  --allowed-origin https://lens.example.com
```

`--auth-token` (env `CTXONE_AUTH_TOKEN`) requires `Authorization: Bearer <tok>`
on non-loopback requests; `--allowed-origin` (env `CTXONE_ALLOWED_ORIGINS`,
comma-separated) allow-lists cross-origin browsers. See
[DEPLOYMENT.md](DEPLOYMENT.md) and [HTTP_API.md](HTTP_API.md#authentication).

### `ctx service [install | uninstall | status]`

Install the unified daemon (`ctxone-hub --http --lens`) as an **always-on
login/boot service** so the hub is up before any agent starts — the recommended
production setup. Uses the platform's native service manager: **launchd**
(macOS), **systemd** (Linux), or **Task Scheduler** (Windows).

```
USAGE: ctx service <install | uninstall | status> [OPTIONS]

  install     Register + start the daemon at login/boot
  uninstall   Stop + remove the service
  status      Show whether the service is installed and running

OPTIONS (install):
      --path <PATH>          Database path  [default: ~/.ctxone/memory.db]
  -p, --port <PORT>          Port  [default: 3001]
      --auth-token <TOK>     Bearer token baked into the service (env: CTXONE_AUTH_TOKEN)
      --no-lens              Don't serve the Lens web UI
```

After `ctx service install`, run `ctx init` to point your AI tools at the
running daemon by URL. See [DEPLOYMENT.md](DEPLOYMENT.md).

### `ctx init [options]`

Auto-detect installed AI tools and write CTXone into their MCP configs.

```
USAGE: ctx init [OPTIONS]

OPTIONS:
      --transport <MODE>     http | stdio  [default: http] — http points tools
                             at a running daemon by URL (one shared process);
                             stdio makes each tool spawn its own hub child
      --global               User-level config instead of project-only
      --project              Project-only (default)
      --tool <NAME>          Target a specific tool: claude, cursor, vscode,
                             codex, gemini, grok
      --config-path <PATH>   Write MCP JSON to an arbitrary file — for MCP
                             clients ctx init doesn't know about yet
      --dry-run              Show what would be written without writing
```

With `--transport http` (the default), `ctx init` writes URL configs and, when
a `--auth-token` is set on the hub, injects the bearer token into each config.
`--transport stdio` writes the per-client spawn shape instead.

**Supported tools** (auto-detection + auto-configuration):
Claude Code, Claude Desktop, Cursor, VS Code (Copilot MCP), Codex,
Gemini CLI, Grok CLI.

**Generic fallback:** use `--config-path` for any MCP client that
`ctx init` doesn't directly support. It writes the standard
`mcpServers` JSON shape to any path you specify, merging with any
existing file.

`ctx init` also prompts to install the `AGENTS.md` guidance unless you
pass `--no-agents`.

### `ctx skill [options]`

Install CTXone's **Agent Skill** (`SKILL.md`) into each detected host's
skills directory — teaches the agent *when* to record decisions, open
plans, and recall context. Version-stamped; won't clobber a newer
on-disk skill. When the `asd` CLI is also on PATH, it additionally
installs the combined **CTXone + ASD** suite skill.

```
USAGE: ctx skill [OPTIONS]

OPTIONS:
      --project      Install project-scoped (into the repo) instead of user-wide
      --tool <KEY>   Only install for one host key (e.g. claude-code)
      --remove       Remove installed skill files instead of writing them
      --status       Report install state without changing anything
      --dry-run      Print what would happen without touching the filesystem
      --no-nudge     Suppress the one-time suggestion to add ASD (also CTX_NO_SUGGEST=1)
      --emit-spec    Print CTX's SkillSpec as JSON and exit (cross-CLI contract
                     for the combined suite skill)
```

### `ctx bootstrap`

Print a paste-into-your-agent block that installs and primes CTXone —
and offers to set up **AgentStateDeveloper** (code context) too. The
fastest path to wiring CTXone into whatever agent you're already in.

### `ctx agents <action>`

Manage the `AGENTS.md` guidance file — a short, pinned document that
teaches AI tools how to use CTXone. `ctx agents show` prints the full
text; `ctx agents install` writes it and primes it as pinned memory
(prompts unless `--yes`). See also [AGENTS.md](AGENTS.md).

### `ctx db backup [--suffix <NAME>]`

Trigger a snapshot of the live db. The hub responds with the path it
wrote (under `<db>.bak.<utc>`). Cheap — runs against a live hub via
SQLite `VACUUM INTO`, no downtime.

```
USAGE: ctx db backup [OPTIONS]

OPTIONS:
      --suffix <NAME>  Override the .bak.<...> suffix (default: UTC)
```

### `ctx db restore <SNAPSHOT> --to <DB_PATH> [--yes]`

Restore the live db from a snapshot. The hub MUST be stopped — this
command checks for an active `<DB_PATH>.lock` and refuses (exit 75)
if a hub is running. The current db is renamed to
`<DB_PATH>.pre-restore-<unix_ts>` so the operation is reversible:
just rename it back.

```
USAGE: ctx db restore <SNAPSHOT> --to <DB_PATH> [OPTIONS]

OPTIONS:
      --to <PATH>  Path to the live db to overwrite (must match what
                   the hub will use on next start)
      --yes        Skip the y/N confirmation prompt
```

### `ctx completion <shell>`

Generate a shell completion script to stdout.

```
USAGE: ctx completion <SHELL>

SHELLS: bash | zsh | fish | powershell | elvish
```

Typical install:

```bash
# zsh
ctx completion zsh > ~/.zfunc/_ctx
echo 'fpath+=(~/.zfunc); autoload -U compinit; compinit' >> ~/.zshrc

# bash
ctx completion bash > /usr/local/etc/bash_completion.d/ctx

# fish
ctx completion fish > ~/.config/fish/completions/ctx.fish
```

---

## Response format details

### `remember` response

```json
{
  "status": "ok",
  "ref": "main",
  "path": "/memory/licensing/18a6...",
  "commit_id": "sg_e762325fed96"
}
```

### `recall` response

```json
{
  "topic": "licensing",
  "ref": "main",
  "results": [
    {
      "path": "/memory/pinned/vision/the-insight",
      "title": "The Insight",
      "body": "...",
      "pinned": true
    },
    {
      "path": "/memory/licensing/18a6...",
      "value": "CTXone uses BSL-1.1",
      "pinned": false,
      "score": 2,
      "full_match": true
    }
  ],
  "pinned_count": 5,
  "topic_matches": 2,
  "ctx_tokens_sent": 620,
  "ctx_tokens_estimated_flat": 1191,
  "ctx_savings_ratio": 1.92
}
```

### `log` response

Array of commits:

```json
[
  {
    "id": "sg_e762325fed96",
    "timestamp": "2026-04-14T17:47:43...",
    "agent_id": "ctxone",
    "confidence": 0.95,
    "intent": {
      "category": "Custom(\"Observe\")",
      "description": "CTXone uses BSL-1.1",
      "tags": []
    },
    "reasoning": null
  }
]
```

### `diff` response

```json
{
  "ref_a": "main",
  "ref_b": "experiment",
  "ops": [
    {
      "op": "AddKey",
      "path": "/memory/test",
      "key": "abc",
      "value": "new fact"
    }
  ]
}
```

Op tags: `SetValue`, `AddKey`, `RemoveKey`, `AppendItem`, `RemoveItem`.

---

## Environment variables

| Variable | Description |
|----------|-------------|
| `CTX_SERVER` | Default value for `--server` |
| `CTX_BRANCH` | Default value for `--branch` |
| `CTX_NAMESPACE` | Default value for `--namespace` (skips project detection) |
| `CTX_FORMAT` | Default value for `--format` |
| `HOME` | Used by `find_hub_binary` and `canonical_db_path` |
| `DATABASE_URL` | When `ctxone-hub` is launched with `--storage postgres` |

Command-line flags always override environment variables.

---

## `ctx plan` — multi-step work tracked across sessions

Plans are CTXone's cure for **plan rot** — the decay that happens
when task state lives in unstructured markdown files. Every plan
operation writes a blameable commit; proofs are required to close
tasks; branches carry their own plans.

### `ctx plan new <name>`

Create a new plan. Name should be kebab-case.

```
$ ctx plan new website-v2 --description "Brand pivot"
Plan created: website-v2
  status: active
```

### `ctx plan add <plan> "<title>"`

Add a task to an existing plan.

```
$ ctx plan add website-v2 "Rewrite hero" \
    --priority high \
    --assigned-to claude-code \
    --blocks t-001
```

Options:

- `--description <text>` — longer-form, appended to title.
- `--priority low|medium|high|critical` (default `medium`)
- `--parent <task-id>` — nest as subtask (one level only)
- `--assigned-to <agent>` — address the task to a specific agent
- `--blocks <task-id>` — task that must be `done` first, repeatable

### `ctx plan start <plan> <task-id>`

Transition `pending → in_progress`. Refuses if any blocker isn't
done yet; error lists the blocking tasks.

### `ctx plan done <plan> <task-id> --proof <spec>`

Transition `in_progress → done`. **Requires** `--proof`.

Proof spec: `kind:value[:note]`. `kind` is one of
`commit` / `file` / `test` / `text`. Examples:

```
--proof "commit:ef6ce63"
--proof "file:src/foo.rs:refactor for clarity"
--proof "test:test_hero_renders"
--proof "text:confirmed in chat"
```

Completing the last open task auto-promotes the plan to `completed`.

### `ctx plan abandon <plan> <task-id> --reason "<text>"`

Record the task as abandoned. `--reason` is required.

### `ctx plan next <plan>`

Show the next pickable task.

Options:

- `--assigned-to <agent>` — filter to tasks addressed to this agent
- `--me` — shortcut for `--assigned-to <session-agent>` (uses
  `CTX_AGENT_ID` or the config default)
- `--include-unassigned` — include unowned tasks alongside assigned
  ones (default on)
- `--assigned-only` — restrict strictly to explicitly assigned tasks
- `--in-order` — for sequential plans, pick the first unstarted task in
  order rather than any pickable one

With `--me`, two agents sharing one plan each pick up their own tasks
without stepping on each other. This is **state-driven orchestration**
— the plan IS the orchestration layer. No framework, no DAG runtime.

### `ctx plan tasks <plan>`

Print the flat task list for a plan — task ids, statuses, titles. Useful
in scripts or to drive a `plan done` loop. Mirrors the `plan_tasks` MCP
tool.

### `ctx plan list`

List plans on the current branch.

Options: `--status active|completed|archived` — filter.

### `ctx plan show <plan>`

Render a plan as a tree with tasks, statuses, proofs, assignments,
and blockers.

### `ctx plan complete <plan> --reason "<text>"`

**Force-complete** an entire plan in one shot — marks every remaining
task done with a "force-complete" reason and closes the plan. Use when
remaining work is no longer relevant (scope cut, plan superseded). For
incremental progress, prefer `ctx plan done` per task.

### `ctx plan archive <plan>`

Soft-archive. Task data preserved.

### `ctx plan link <plan> <task-id> <plan/task>`

Record that this task, when done, **satisfies a task in another plan** (a
cross-plan pointer). Advisory: `plan done` on the linked task then reminds you
to close the target. Also on `plan next --in-order` (pick the first unstarted
task by order instead of highest priority).

### `ctx plan stale [--days N]`

List **in-progress tasks idle for N days** (default 7), most-stale first — the
drift `plan start`'s warning guards against. Add `--all-namespaces` for a global
view.

---

## `ctx docs` — canonical-doc registry

Index your canonical `.md` docs so agents find the right one without scanning
the repo (the repo file stays canonical; this stores the pointer).

- `ctx docs add <path> [--status --scope --owner --answers --supersedes]` —
  register/update a doc (idempotent per path).
- `ctx docs list` — list registered docs.
- `ctx docs find <query>` — find docs by path/scope/answers/owner.

To import a doc's *content* into recall instead, use `ctx import-doc <file>`
(alias of `ctx prime`; `--pin` to always-include).

## `ctx db export` / `ctx db import`

Portable JSON snapshots (vs the binary `db backup`). `ctx db export [--out FILE]`
dumps the current branch's graph; `ctx db import FILE` merges it back (upsert).
Prune the JSON between the two to seed a fresh db with only what you want.
Branch/namespace scoped via the usual flags.

---

## See also

- [QUICKSTART.md](QUICKSTART.md) — 5-minute get-running guide
- [COOKBOOK.md](COOKBOOK.md) — practical recipes
- [ARCHITECTURE.md](ARCHITECTURE.md) — how recall ranks, how branches work
- [TROUBLESHOOTING.md](TROUBLESHOOTING.md) — common errors and fixes
- [HTTP_API.md](HTTP_API.md) — REST endpoints for non-CLI integrations
- [MCP_TOOLS.md](MCP_TOOLS.md) — MCP tools exposed to agents

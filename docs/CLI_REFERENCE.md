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
| `--branch <REF>` | `CTX_BRANCH` | `main` | Branch / ref to read and write |
| `--format <FMT>` | `CTX_FORMAT` | `text` | Output format: `text`, `json`, or `id` |
| `--help` / `-h` | | | Print help |
| `--version` / `-V` | | | Print version |

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

---

## Operations commands

### `ctx status`

One-line Hub health check plus session token summary.

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
  -p, --port <PORT>        Port  [default: 3001]
      --storage <TYPE>     sqlite | postgres | memory  [default: sqlite]
      --path <PATH>        Database path  [default: ~/.ctxone/memory.db]
      --http               Also start HTTP API (otherwise stdio MCP only)
```

### `ctx init [options]`

Auto-detect installed AI tools and write CTXone into their MCP configs.

```
USAGE: ctx init [OPTIONS]

OPTIONS:
      --global               User-level config instead of project-only
      --project              Project-only (default)
      --tool <NAME>          Target a specific tool: claude, cursor, vscode,
                             codex, gemini, grok
      --config-path <PATH>   Write MCP JSON to an arbitrary file — for MCP
                             clients ctx init doesn't know about yet
      --dry-run              Show what would be written without writing
```

**Supported tools** (auto-detection + auto-configuration):
Claude Code, Claude Desktop, Cursor, VS Code (Copilot MCP), Codex,
Gemini CLI, Grok CLI.

**Generic fallback:** use `--config-path` for any MCP client that
`ctx init` doesn't directly support. It writes the standard
`mcpServers` JSON shape to any path you specify, merging with any
existing file.

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
| `CTX_FORMAT` | Default value for `--format` |
| `HOME` | Used by `find_hub_binary` and `canonical_db_path` |
| `DATABASE_URL` | When `ctxone-hub` is launched with `--storage postgres` |

Command-line flags always override environment variables.

---

## See also

- [QUICKSTART.md](QUICKSTART.md) — 5-minute get-running guide
- [COOKBOOK.md](COOKBOOK.md) — practical recipes
- [ARCHITECTURE.md](ARCHITECTURE.md) — how recall ranks, how branches work
- [TROUBLESHOOTING.md](TROUBLESHOOTING.md) — common errors and fixes
- [HTTP_API.md](HTTP_API.md) — REST endpoints for non-CLI integrations
- [MCP_TOOLS.md](MCP_TOOLS.md) — MCP tools exposed to agents

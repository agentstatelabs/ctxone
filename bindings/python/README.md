# ctxone — Python client for CtxOne

Persistent, searchable, accountable memory for AI agents — from Python.

```bash
pip install ctxone
```

## Quick start

You need a running CtxOne Hub. See the [CtxOne quickstart](https://github.com/ctxone/ctxone/blob/main/docs/QUICKSTART.md)
or install the CLI and run `ctx serve --http`.

```python
from ctxone import Hub

hub = Hub()  # defaults to http://localhost:3001

# Store a fact
hub.remember("We use BSL-1.1 for all projects",
             importance="high",
             context="legal")

# Retrieve relevant memories, with pinned context always included first
result = hub.recall("licensing")
for r in result.results:
    if r.pinned:
        print(f"[PINNED] {r.title}")
    else:
        print(f"- {r.value}")

print(f"\n{result.ctx_tokens_sent} tokens sent, "
      f"{result.ctx_savings_ratio:.1f}x savings vs flat memory")
```

Output:

```
- We use BSL-1.1 for all projects

13 tokens sent, 18.4x savings vs flat memory
```

## Why use this instead of the CLI?

The CLI (`ctx`) is the primary surface for interactive work. This library
is for:

- Scripts, pipelines, and automation that would otherwise shell out
- Jupyter notebooks and data-science workflows
- Wrapping CtxOne inside another Python application
- Typed return values and IDE autocomplete

The library wraps the same HTTP API the CLI uses, so everything works the
same way and talks to the same Hub.

## Features

- **`remember` / `recall`** — store and retrieve facts
- **`prime` / `prime_markdown`** — load structured content as pinned or
  searchable memory
- **`forget`** — delete specific memories
- **Branches** — `create_branch`, `branches`, `merge`, `diff`
- **Graph visibility** — `search`, `ls`, `get`, `log`, `blame`
- **Typed results** — `RecallResult`, `Commit`, `TokenStats`, etc.
- **Error types** — `HubUnreachable`, `NotFound`, `MergeConflict`, `CtxOneError`

## Examples

### Prime a project README as pinned context

```python
with open("README.md") as f:
    hub.prime_markdown("project", f.read(), pinned=True)
```

Every future `hub.recall()` call will include those sections, regardless
of the topic searched.

### Capture a commit id from remember

```python
commit = hub.remember("Shipped v0.55.0")
print(commit.path)        # /memory/facts/<timestamp>
print(commit.commit_id)   # sg_abc123
```

### Branch, experiment, merge

```python
hub.create_branch("experiment")
hub.remember("Trying a new architecture", ref="experiment", context="notes")

ops = hub.diff("main", "experiment")
print(f"{len(ops)} changes on experiment")

# If happy with the experiment, merge it back
commit = hub.merge("experiment", into="main")
```

### Search and inspect

```python
# Literal search across all values
hits = hub.search("BSL")
for hit in hits:
    print(hit["path"], "-", hit["value"])

# Recent commit history with provenance
for commit in hub.log(limit=10):
    print(f"{commit.timestamp[:19]} [{commit.category}] {commit.description}")

# Provenance chain for a specific path
blame = hub.blame("/memory/legal/abc123")
```

### Token savings telemetry

```python
stats = hub.stats()
print(f"Tokens sent: {stats.session_tokens_used}")
print(f"Tokens saved: {stats.session_tokens_saved}")
print(f"Cumulative ratio: {stats.cumulative_ratio:.1f}x")
```

## Error handling

```python
from ctxone import Hub, HubUnreachable, MergeConflict, NotFound

hub = Hub()

try:
    hub.recall("topic")
except HubUnreachable:
    print("Is the Hub running?")

try:
    hub.merge("feature", into="main")
except MergeConflict as e:
    for c in e.conflicts:
        print(f"Conflict at {c['path']}")
```

## Configuration

The `Hub()` constructor accepts:

- `server` — Hub URL (defaults to `CTX_SERVER` env var, then `http://localhost:3001`)
- `branch` — default branch for reads and writes (defaults to `"main"`)
- `timeout` — request timeout in seconds (default 30.0)
- `session` — optional `requests.Session` for custom headers, auth, etc.

```python
# Point at a shared team Hub
hub = Hub(server="http://ctxone.internal.example.com:3001")

# Work on an experiment branch by default
hub = Hub(branch="experiment")

# Custom session with auth header
import requests
session = requests.Session()
session.headers["Authorization"] = "Bearer ..."
hub = Hub(session=session)
```

## Open WebUI integration

The `ctxone.integrations.openwebui` module ships both a **Tool** (`remember`,
`recall`, `forget`, `list_pinned`) and a **Filter** (auto-injects relevant
memory into every chat turn via `inlet`, optionally captures assistant
replies in `outlet`). Install with:

```bash
pip install "ctxone[openwebui]"
```

Or paste the whole
[`src/ctxone/integrations/openwebui.py`](src/ctxone/integrations/openwebui.py)
file into Open WebUI → Admin Panel → Functions — the docstring frontmatter
handles auto-install. Full docs:
[OPENWEBUI.md](https://github.com/ctxone/ctxone/blob/main/docs/OPENWEBUI.md).

## Development

```bash
cd bindings/python
pip install -e ".[dev]"
pytest
```

Integration tests spin up a real `ctxone-hub` process — they're skipped
automatically if the binary isn't available. Build the workspace first
with `cargo build -p ctxone-hub` or set `CTXONE_HUB_BIN` to point at an
installed binary.

## License

BSL-1.1 (same as the rest of CtxOne). Every version automatically
converts to Apache-2.0 four years after release. See
[LICENSING.md](https://github.com/ctxone/ctxone/blob/main/LICENSING.md).

## Links

- [CtxOne repo](https://github.com/ctxone/ctxone)
- [Architecture](https://github.com/ctxone/ctxone/blob/main/docs/ARCHITECTURE.md)
- [Token Savings](https://github.com/ctxone/ctxone/blob/main/docs/TOKEN_SAVINGS.md)
- [CLI Reference](https://github.com/ctxone/ctxone/blob/main/docs/CLI_REFERENCE.md)

# HTTP API Reference

The CtxOne Hub exposes a REST API over HTTP when run with `--http`. This
doc lists every endpoint, its request format, response format, and any
query parameters.

All endpoints live under `http://<host>:<port>/api/`. Default host and port:
`0.0.0.0:3001`. CORS is enabled with `Allow-Origin: *`.

## Conventions

- **Branch/ref parameter:** most endpoints take a branch name in the URL
  path (`{ref_name}`) or as a `ref` query string / body field. Defaults to
  `main`.
- **Content type:** requests and responses use `application/json`.
- **Error responses:** HTTP 4xx for bad input, 5xx for server errors. Body
  is plain text with a human-readable message.

## Health

### `GET /api/health`

Simple liveness check.

**Response (200):**
```json
{
  "status": "ok",
  "service": "ctxone-hub"
}
```

Used by `ctx status` and `ctx doctor`.

---

## Stats

### `GET /api/stats/tokens`

Cumulative session token savings.

**Response (200):**
```json
{
  "session_tokens_used": 98,
  "session_tokens_saved": 1706,
  "total_graph_size_chars": 1804,
  "total_graph_size_tokens": 451,
  "cumulative_ratio": 18.43
}
```

- `session_tokens_used` — total tokens actually sent in responses this Hub
  session
- `session_tokens_saved` — `(number_of_recalls × flat_baseline) - used`
- `total_graph_size_chars` — raw character count of the serialized graph
- `total_graph_size_tokens` — chars ÷ 4 (rough estimate)
- `cumulative_ratio` — `(used + saved) / used`

### `GET /api/stats/{ref_name}`

Structural stats for a branch.

**Response (200):**
```json
{
  "commit_count": 27,
  "path_count": 21,
  "branch_count": 2,
  "epoch_count": 0,
  "agents": ["ctxone", "ctxone-prime"],
  "categories": ["Checkpoint", "Custom(\"Observe\")"],
  "latest_commit": {
    "id": "sg_e762325fed96",
    "timestamp": "2026-04-14T17:47:43Z",
    "agent": "ctxone",
    "intent": "fact description"
  }
}
```

---

## Read endpoints (state)

### `GET /api/state/{ref_name}?path=<path>`

Read a value at a specific path.

**Query params:**
- `path` — JSON path to read (default: `/`)

**Response (200):** the value at that path, pretty-printed JSON.

### `GET /api/state/{ref_name}/paths?prefix=<prefix>&max_depth=<n>`

List all paths under a prefix.

**Query params:**
- `prefix` — path prefix (default: `/`)
- `max_depth` — max tree depth (default: 50)

**Response (200):** array of path strings.

```json
["/memory/licensing/abc", "/memory/architecture/def", ...]
```

### `GET /api/state/{ref_name}/search?query=<q>&max_results=<n>`

Literal substring search across values and keys.

**Query params:**
- `query` — substring to match (case-insensitive)
- `max_results` — max results (default: 50)

**Response (200):**
```json
[
  {"path": "/memory/licensing/abc", "value": "CtxOne uses BSL-1.1"},
  ...
]
```

---

## Log and blame

### `GET /api/log/{ref_name}?limit=<n>`

Recent commit history.

**Query params:**
- `limit` — max commits (default: 20)

**Response (200):** array of commits. See the `log` response schema in
[CLI_REFERENCE.md](CLI_REFERENCE.md#log-response).

### `GET /api/blame/{ref_name}?path=<path>`

Provenance chain for a specific path.

**Query params:**
- `path` — path to blame

**Response (200):** array of blame entries with commit id, agent,
timestamp, intent, and confidence.

### `GET /api/diff?ref_a=<a>&ref_b=<b>`

Diff two refs.

**Query params:**
- `ref_a` — first ref (usually older / base)
- `ref_b` — second ref (usually newer / target)

**Response (200):**
```json
{
  "ref_a": "main",
  "ref_b": "experiment",
  "ops": [
    {"op": "AddKey", "path": "/memory/test", "key": "abc", "value": "..."},
    {"op": "SetValue", "path": "/...", "old": {...}, "new": {...}},
    {"op": "RemoveKey", "path": "/...", "key": "..."}
  ]
}
```

Op tags: `SetValue`, `AddKey`, `RemoveKey`, `AppendItem`, `RemoveItem`.

---

## Branches

### `GET /api/branches`

List all branches.

**Response (200):**
```json
[
  {"name": "main", "id": "sg_e762..."},
  {"name": "experiment", "id": "sg_a3b1..."}
]
```

### `POST /api/branches`

Create a new branch.

**Request body:**
```json
{
  "name": "experiment",
  "from": "main"
}
```

**Response (200):**
```json
{
  "status": "ok",
  "name": "experiment",
  "from": "main",
  "commit_id": "sg_a3b1..."
}
```

---

## Memory endpoints (the high-level API)

These are the endpoints CtxOne's memory layer adds on top of the underlying
state primitives.

### `POST /api/memory/remember`

Store a fact.

**Request body:**
```json
{
  "fact": "CtxOne uses BSL-1.1 licensing",
  "importance": "high",
  "context": "licensing",
  "tags": ["legal", "decision"],
  "ref": "main"
}
```

- `fact` (required) — the string to store
- `importance` — `high` / `medium` / `low` (default `medium`). Maps to
  confidence 0.95/0.7/0.4.
- `context` — category name; storage path is `/memory/<context>/<id>`
- `tags` — queryable tags stored on the commit
- `ref` — branch to write to (default `main`)

**Response (200):**
```json
{
  "status": "ok",
  "ref": "main",
  "path": "/memory/licensing/18a6...",
  "commit_id": "sg_e762..."
}
```

### `POST /api/memory/forget`

Delete a memory at a specific path.

**Request body:**
```json
{
  "path": "/memory/licensing/18a6...",
  "reason": "superseded by new policy",
  "ref": "main"
}
```

Marked in blame as a `Rollback` intent with the given reason.

**Response (200):**
```json
{
  "status": "ok",
  "ref": "main",
  "path": "/memory/licensing/18a6...",
  "commit_id": "sg_next..."
}
```

### `GET /api/memory/recall?topic=<t>&budget=<n>&ref=<r>`

Retrieve memories for a topic. Pinned-first, token-scored, budget-capped.

**Query params:**
- `topic` — query string (tokenized, multi-word supported)
- `budget` — max token budget (default 1500)
- `ref` — branch (default `main`)

**Response (200):** see the `recall` response schema in
[CLI_REFERENCE.md](CLI_REFERENCE.md#recall-response).

Every recall updates the session token counters — each call's `sent`
contributes to `session_tokens_used` on `GET /api/stats/tokens`.

### `GET /api/memory/context/{project}?ref=<r>`

Load the full context tree for a project.

**Response (200):**
```json
{
  "project": "myproject",
  "ref": "main",
  "context": {
    "status": "active",
    "decisions": {...}
  },
  "ctx_tokens_sent": 234,
  "ctx_tokens_estimated_flat": 1191
}
```

### `POST /api/memory/prime`

Load structured sections as pinned or searchable memory.

**Request body:**
```json
{
  "source": "project",
  "pinned": true,
  "sections": [
    {"title": "The Insight", "body": "..."},
    {"title": "The Roadmap", "body": "..."}
  ],
  "ref": "main"
}
```

- `source` (required) — group name; re-priming the same source overwrites
- `pinned` — if true, always include in recall; otherwise searchable (default false)
- `sections` — parsed markdown sections from the client
- `ref` — branch (default `main`)

**Response (200):**
```json
{
  "status": "ok",
  "ref": "main",
  "source": "project",
  "pinned": true,
  "sections_written": 5,
  "paths": [
    "/memory/pinned/project/the-insight",
    "/memory/pinned/project/the-roadmap",
    ...
  ]
}
```

### `GET /api/memory/pinned`

List all pinned memories.

**Response (200):**
```json
[
  {"path": "/memory/pinned/project/the-insight/title", "value": "The Insight"},
  {"path": "/memory/pinned/project/the-insight/body", "value": "..."},
  ...
]
```

Clients typically group these by `/memory/pinned/<source>/<slug>` and pair
the `/title` and `/body` children to reconstruct structured sections.
Returns an empty array (not 404) when no pinned memories exist.

### `POST /api/memory/summarize_session`

End-of-session commit capturing what was learned.

**Request body:**
```json
{
  "session_id": "2026-04-14-afternoon",
  "key_points": ["Shipped Postgres backend", "Built auth middleware"],
  "decisions": ["SaaS as on-ramp", "agent memory is top priority"]
}
```

**Response (200):**
```json
{
  "status": "ok",
  "session_id": "2026-04-14-afternoon",
  "key_points": 2,
  "decisions": 2
}
```

### `GET /api/memory/what_changed_since?since=<iso>`

Recent commits filtered to those after a timestamp.

**Query params:**
- `since` — ISO 8601 timestamp (e.g., `2026-04-12T00:00:00Z`)

**Response (200):** array of commit summaries.

### `GET /api/memory/why_did_we?decision=<text>`

Search for a decision and return its blame chain.

**Query params:**
- `decision` — substring of the decision to look up

**Response (200):**
```json
{
  "decision": "use BSL-1.1",
  "traces": [
    {
      "path": "/memory/licensing/abc",
      "blame": [...]
    }
  ]
}
```

---

## Error responses

| Status | Meaning | Example body |
|--------|---------|--------------|
| 400 | Malformed request (missing required field) | `"missing field \`fact\`"` |
| 404 | Path or ref not found | `"ref not found: experiment"` |
| 500 | Internal error (storage, engine) | `"tree error: ..."` |

The body is plain text, not JSON. Clients should log and retry on 5xx.

---

## Rate limiting and auth

The HTTP API currently has **no authentication or rate limiting**. Run the
Hub on a trusted network (loopback, VPN, or private subnet) or put a
reverse proxy in front.

Multi-tenant auth is tracked as future work — see the engine's
`agentstategraph-mcp` binary, which supports `--auth` and `--keys-file`
for tenant isolation. CtxOne Hub doesn't currently expose these.

---

## See also

- [CLI_REFERENCE.md](CLI_REFERENCE.md) — the `ctx` CLI, which wraps this API
- [MCP_TOOLS.md](MCP_TOOLS.md) — the MCP tools, which wrap the same underlying logic
- [ARCHITECTURE.md](ARCHITECTURE.md) — how recall ranks, how the graph is structured

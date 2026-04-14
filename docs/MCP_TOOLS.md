# MCP Tools Reference

The CtxOne Hub exposes 7 MCP tools over the stdio transport. Any
MCP-compatible agent (Claude Code, Cursor, VS Code Copilot with MCP,
Codex, etc.) can call these directly.

For setup instructions, see [INTEGRATIONS.md](INTEGRATIONS.md).
For the underlying concepts, see [ARCHITECTURE.md](ARCHITECTURE.md).

## Connecting

In your AI tool's MCP config:

```json
{
  "mcpServers": {
    "ctxone": {
      "command": "/path/to/ctxone-hub",
      "args": ["--path", "/Users/user/.ctxone/memory.db"]
    }
  }
}
```

`ctx init` writes this config for you automatically in every detected
tool.

The Hub runs in stdio mode when invoked without `--http`. It stays alive
for the duration of the agent session and handles one client at a time.

## Tools

All tools return **structured text** (usually JSON). Agents parse the
response and decide what to do with it.

Every response from `remember`, `recall`, `context`, `summarize_session`,
`what_changed_since`, and `why_did_we` carries token usage metadata in an
`_ctxone_stats` trailer (or embedded fields for the JSON-native tools).

---

### `remember`

Store a fact, preference, or decision.

**Description (from the tool descriptor):**
> Store a fact, preference, or decision in agent memory. Facts are
> searchable and carry confidence scores based on importance.

**Parameters:**

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `fact` | string | yes | — | The fact to store |
| `importance` | enum | no | `medium` | `high` / `medium` / `low` |
| `context` | string | no | — | Category (becomes `/memory/<context>/<id>`) |
| `tags` | string[] | no | — | Queryable tags |
| `ref` | string | no | `main` | Branch to write to |

**Response:** JSON object with `status`, `path`, `commit_id`, `ref`, `fact`.

**When to call:** any time the agent learns a fact about the user's project
that should persist to the next session. Agents are encouraged to call this
liberally — the more facts, the better recall ranking gets.

---

### `recall`

Retrieve memories for a topic.

**Description:**
> Retrieve relevant memories for a topic. Always includes pinned context
> first, then topic-matched facts, respecting a token budget. Response is
> JSON including token savings metadata.

**Parameters:**

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `topic` | string | yes | — | Query string (tokenized) |
| `budget` | integer | no | 1500 | Max token budget |
| `ref` | string | no | `main` | Branch to read |

**Response:** JSON with `results`, `pinned_count`, `topic_matches`,
`ctx_tokens_sent`, `ctx_tokens_estimated_flat`, `ctx_savings_ratio`.

Results are structured: each item has `path` and `pinned: bool`. Pinned
items also have `title` and `body`; topic matches have `value`, `score`,
and `full_match`.

**When to call:** at the start of every agent session, and whenever the
agent needs project-specific context mid-session. Think of it as "search
my memory for anything relevant to <topic>" — the agent should call it
proactively, not just when the user asks.

---

### `prime`

Load structured sections as pinned or primed memory.

**Description:**
> Load markdown sections as pinned or primed memories. Pinned memories are
> always included in every recall response (critical context). Sections
> should be pre-parsed — each entry has a title and body.

**Parameters:**

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `source` | string | yes | — | Group name (idempotent) |
| `pinned` | bool | no | false | If true, always include in recall |
| `sections` | array | yes | — | Array of `{title, body}` objects |
| `ref` | string | no | `main` | Branch to write to |

**Response:** JSON with `sections_written` count and `paths` array.

**When to call:** when loading a new document into memory. Agents
typically parse a markdown file on the client side (H1/H2 headings) and
pass the parsed sections. The CLI's `ctx prime` does exactly this.

Use `pinned: true` for critical context (project conventions, current
status) and `pinned: false` for searchable reference material.

---

### `context`

Load the full context tree for a project.

**Description:**
> Load the full context tree for a specific project or domain. Returns all
> stored state under that project path.

**Parameters:**

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `project` | string | yes | — | Project name (reads `/memory/projects/<project>`) |
| `ref` | string | no | `main` | Branch to read |

**Response:** JSON with the full subtree serialized. Includes token stats.

**When to call:** at the start of a session when the user specifies a
project, to dump everything under that project into context in one call.

---

### `summarize_session`

End-of-session commit.

**Description:**
> End-of-session commit capturing what was learned and decided. Call this
> before closing a session to persist its knowledge.

**Parameters:**

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `session_id` | string | yes | — | Unique identifier for this session |
| `key_points` | string[] | yes | — | Bullet points of what was learned |
| `decisions` | string[] | no | `[]` | Decisions made this session |

Writes three paths:
- `/sessions/<id>/summary` — joined key points (Checkpoint, 0.9 confidence)
- `/sessions/<id>/decisions` — decisions array (Checkpoint, 0.95 confidence)
- `/sessions/<id>/details` — full key points (Observe)

**When to call:** at session end, before the agent shuts down. The
corresponding `recall` on the next session will find these summaries,
enabling the "close session, open new one, context preserved" workflow.

---

### `what_changed_since`

Recent commits filtered by timestamp.

**Description:**
> See what has changed in the memory graph since a given date. Shows recent
> commits and their intents.

**Parameters:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `since` | string | yes | ISO 8601 timestamp (e.g., `2026-04-12T00:00:00Z`) |

**Response:** text listing of recent commits with timestamp, category,
description, and confidence.

**When to call:** at session start when the agent wants to catch up on
what's happened since the last session.

---

### `why_did_we`

Trace the reasoning behind a past decision.

**Description:**
> Trace the reasoning behind a past decision. Searches for the decision and
> returns its full provenance chain (blame).

**Parameters:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `decision` | string | yes | Substring of the decision (e.g., "use BSL 1.1") |

**Response:** text with matched paths and their blame chains (commit
history showing who/when/why).

**When to call:** when the user asks "why did we decide X?" or the agent
needs to justify a past choice to the user.

---

## Token stats trailer

Tools that return plain text (most of the older ones) append a stats line:

```
<response body>

_ctxone_stats: {"ctx_tokens_sent":42,"ctx_tokens_estimated_flat":451,"ctx_savings_ratio":10.7}
```

Tools that return JSON natively (`remember`, `recall`, `prime`, `context`)
embed the same fields directly in the response object.

Well-behaved agents can extract and surface these numbers to the user —
CtxOne is the only memory layer where "how much did this query save?" is a
first-class API response.

## Authority chain

Every tool call writes commits with `agent_id = "ctxone"` (or
`"ctxone-prime"` for prime operations). This means `ctx blame` shows
CtxOne-mediated writes separately from writes via the raw engine CLI.

If you're running multiple agents that share a Hub, consider giving each
agent its own namespaced branch (`agents/alice`, `agents/bob`) so blame is
unambiguous.

---

## See also

- [HTTP_API.md](HTTP_API.md) — same logic exposed over REST
- [INTEGRATIONS.md](INTEGRATIONS.md) — how to wire these tools into specific AI clients
- [ARCHITECTURE.md](ARCHITECTURE.md) — the underlying graph model

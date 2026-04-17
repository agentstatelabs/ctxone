# MCP Tools Reference

The CTXone Hub exposes its memory primitives and the plan primitives
as MCP tools over the stdio transport. Any MCP-compatible agent
(Claude Code, Cursor, VS Code Copilot with MCP, Codex, etc.) can call
these directly.

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

### `record_llm_usage`

Report the LLM turn's token usage to CTXone.

**Description:**
> Report LLM token usage to CTXone for metrics and cost accounting.
> Call this after any significant LLM turn — pass the numbers
> straight from the model's response `usage` field.

**Parameters:**

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `input_tokens` | integer | yes | — | Tokens the model consumed as input |
| `output_tokens` | integer | yes | — | Tokens the model generated |
| `cache_read_tokens` | integer | no | `0` | Tokens served from prompt cache (Anthropic) |
| `cache_create_tokens` | integer | no | `0` | Tokens written to prompt cache (Anthropic) |
| `model` | string | no | — | Model identifier for display (e.g. `claude-sonnet-4.5`) |
| `provider` | string | no | — | Provider identifier (`anthropic`, `openai`, `gemini`, …) |

**Response:** JSON object with the updated per-session totals
(`llm_input_tokens`, `llm_output_tokens`, `llm_cache_read_tokens`,
`llm_cache_create_tokens`, `llm_call_count`, `last_model`,
`last_provider`).

**When to call:** after every LLM turn where you actually invoked a
model. The agent just copies numbers out of the provider response's
`usage` field into the call parameters. Don't invent numbers, and
don't call for trivial housekeeping turns.

**Why it matters:** CTXone's internal savings ratio is computed from
what the Hub itself sent in recall responses — an extrapolation.
This tool gives Lens ground-truth measurements of actual model
consumption, cache hit ratios, and real dollar cost. Sessions that
report LLM usage render with real numbers in Lens; sessions that
don't fall back to the CTXone-side view only.

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
CTXone is the only memory layer where "how much did this query save?" is a
first-class API response.

## Authority chain

Every tool call writes commits with `agent_id = "ctxone"` (or
`"ctxone-prime"` for prime operations). This means `ctx blame` shows
CTXone-mediated writes separately from writes via the raw engine CLI.

If you're running multiple agents that share a Hub, consider giving each
agent its own namespaced branch (`agents/alice`, `agents/bob`) so blame is
unambiguous.

---

## Plan tools

Ten MCP tools wrap the plan primitives from the
`agentstategraph-tasks` crate and surface them with proactive
"CALL THIS WHEN" descriptions. Plans persist under `/plans/<name>/`
and survive session boundaries — the same plan can be picked up by
another agent or by you tomorrow.

### `plan_new`

Create a plan.

**When to call:** the user describes a multi-step task. Don't ask
permission — if the work is multi-step, plan it.

**Parameters:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Kebab-case plan name |
| `description` | string | no | One or two sentences |
| `ref` | string | no | Branch, default `main` |

Returns the created plan as JSON.

---

### `plan_add`

Add a task to a plan.

**When to call:** enumerating the steps of a multi-step task — add
every step as a task before you start executing.

**Parameters:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `plan_id` | string | yes | Plan name |
| `title` | string | yes | Imperative sentence |
| `description` | string | no | Longer-form (appended to title) |
| `priority` | enum | no | `low` / `medium` / `high` / `critical` |
| `parent_id` | string | no | Parent task id for a subtask |
| `assigned_to` | string | no | Agent id (e.g. `claude-code`, `codex`) |
| `blocked_by` | string[] | no | Task ids that must be done first |
| `ref` | string | no | Branch, default `main` |

Passing `assigned_to` enables the state-driven orchestration pattern —
see `plan_next` below.

---

### `plan_start`

Transition `pending → in_progress`.

**When to call:** you begin working on a task. Refuses with an error
listing the blockers if any entry in `blocked_by` is not yet `done`.

**Parameters:** `plan_id`, `task_id`, `reason?`, `ref?`.

---

### `plan_complete`

Transition `in_progress → done`. Requires a proof.

**When to call:** you finish a task. Proof kinds in order of
preference:

- `commit` — a git SHA (strongest)
- `file` — a path you created or edited
- `test` — a test name that now exists or passes
- `text` — human-attested last-resort

**Parameters:** `plan_id`, `task_id`, `proof` ({kind, value, note?}),
`reason?`, `ref?`.

Completing the last open task in a plan auto-promotes the plan to
`completed`.

---

### `plan_abandon`

Mark a task as abandoned. Requires a reason.

**When to call:** a task turns out to be unnecessary, superseded, or
no longer wanted. Legal from both `pending` and `in_progress`.

**Parameters:** `plan_id`, `task_id`, `reason`, `ref?`.

---

### `plan_next`

Return the highest-priority pickable task.

**When to call:** you need to know what to work on next.

**Parameters:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `plan_id` | string | yes | Plan name |
| `assigned_to` | string | no | Agent id, or `"me"` |
| `include_unassigned` | bool | no | Default `true` |
| `assigned_only` | bool | no | Default `false` |
| `ref` | string | no | Branch |

Pass `assigned_to="me"` to filter to tasks addressed to you. This is
the state-driven orchestration primitive: two agents with different
agent ids each call `plan_next(assigned_to="me")` and pick up their
own work without stepping on each other. Without `assigned_to`, any
agent sees any pickable task.

Returns the task object or `null`.

---

### `plan_list`

List plans on the branch.

**When to call:** at the start of any session where you might be
resuming prior work.

**Parameters:** `status_filter?` (`active` / `completed` / `archived`),
`ref?`.

---

### `plan_get`

Fetch a plan with its full task list.

**Parameters:** `plan_id`, `ref?`.

---

### `plan_tasks`

List the tasks of a plan, flat.

**Parameters:** `plan_id`, `ref?`.

---

### `plan_archive`

Set plan status to `archived`. Soft — task data is preserved.

**Parameters:** `plan_id`, `ref?`.

---

## See also

- [HTTP_API.md](HTTP_API.md) — same logic exposed over REST
- [INTEGRATIONS.md](INTEGRATIONS.md) — how to wire these tools into specific AI clients
- [ARCHITECTURE.md](ARCHITECTURE.md) — the underlying graph model
- [AGENTS.md](AGENTS.md) — guidance on when to reach for plans vs. inline work

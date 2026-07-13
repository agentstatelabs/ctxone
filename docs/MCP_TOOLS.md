# MCP Tools Reference

The CTXone Hub exposes **56 MCP tools**, served over HTTP at `/mcp` (the
standard setup) and over stdio, in eight groups:

- **Memory** (8): `remember`, `recall`, `prime`, `context`,
  `summarize_session`, `what_changed_since`, `why_did_we`,
  `project_status`
- **Plans** (14): `plan_new`, `plan_add`, `plan_start`,
  `plan_done`, `plan_abandon`, `plan_next`, `plan_list`,
  `plan_show`, `plan_tasks`, `plan_link`, `plan_stale`, `plan_move`,
  `plan_complete`, `plan_archive`
- **Docs** (1): `docs_find`
- **Reminders** (9): `reminder_create`, `remind_me`, `reminder_list`,
  `reminder_get`, `reminder_snooze`, `reminder_approve`, `reminder_cancel`,
  `reminder_start`, `reminder_record`
- **Governance** (8): `forget`, `branches`, `branch`, `merge`,
  `taint_list`, `taint_check`, `taint_apply`, `taint_remove`
- **Read primitives** (6): `get`, `ls`,
  `search`, `log`, `blame`, `diff`
- **Code intelligence** (9): `code_repos`, `code_search`, `code_read`,
  `callers_of`, `callees_of`, `get_active_repo`, `set_active_repo`,
  `code_cross_repo_edges`, `code_impact`
  _(requires `--asd-repo` or `--asd-url` at hub startup; the federated
  `code_cross_repo_edges` / `code_impact` shell out to the `asd` CLI over the
  shared repo registry)_
- **Accounting** (1): `record_llm_usage`

Any MCP-compatible agent (Claude Code, Cursor, VS Code Copilot with
MCP, Codex, etc.) can call these directly.

For setup instructions, see [INTEGRATIONS.md](INTEGRATIONS.md).
For the underlying concepts, see [ARCHITECTURE.md](ARCHITECTURE.md).

## Connecting

**Standard (HTTP daemon).** Point your tool at a running `ctxone-hub --http`
by URL — one daemon serves MCP + REST + Lens (see [ARCHITECTURE.md](ARCHITECTURE.md)):

```json
{ "mcpServers": { "ctxone": {
  "type": "http",
  "url": "http://localhost:3001/mcp?namespace=<detected>"
} } }
```

**Fallback (stdio).** The Hub also speaks MCP over stdio when spawned per-client
without `--http` — it stays alive for the agent session and handles one client
at a time:

```json
{ "mcpServers": { "ctxone": {
  "command": "/path/to/ctxone-hub",
  "args": ["--path", "/Users/user/.ctxone/memory.db"]
} } }
```

`ctx init` writes the right shape for you automatically in every detected tool
(defaulting to the http daemon; `--transport stdio` for the fallback).

## Namespace & branch mirroring

Every stdio session is scoped to a **namespace** — the per-repo
isolation unit for branches, plans, memory, taints, reminders, and
history (see [MEMORY_BRANCH_SCOPING.md](MEMORY_BRANCH_SCOPING.md)).
The namespace is resolved once, at server startup:

1. An explicit `--namespace <ns>` flag or `CTX_NAMESPACE` env var wins.
2. Otherwise the project detection chain runs from the process cwd
   (the spawning tool starts the MCP server in the project directory):
   a `.ctxproject` file in the cwd or any parent, then the repo's
   `git remote get-url origin` looked up in the Hub's project registry.
3. No match → the reserved `default` namespace.

The repository is forked to the resolved namespace for the whole
session — every tool call reads and writes inside it.

**Branch mirroring** happens at startup too: inside a project
namespace, the session's default ref becomes the sanitized current git
branch (`feature/x` → `feature-x`), auto-created from `main` on first
use. Tools that omit their `ref`/branch parameter use this session
default; explicit refs are unchanged. Detached HEAD → no mirroring,
default stays `main`.

Call `project_status` at any time to see the resolved namespace,
agent id, and default ref.

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

**Response:** JSON object with `status`, `path`, `commit_id`, `ref`,
`namespace` (which project namespace the write landed in), `fact`.

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

### `project_status`

Show which project namespace this session operates in.

**Description:**
> Show which project namespace this session's memory operations land in,
> plus the agent id stamped on commits. Call this to prove where a write
> went, or to debug why remembered facts seem missing (usually: they were
> written in a different namespace).

**Parameters:** none.

**Response:** JSON with `namespace`, `agent_id`, `default_ref` (the
session's mirrored git branch, or `main`), and a `hint` string. In the
`default` namespace the hint suggests running `ctx project add <id>` in
the repo (or committing its `.ctxproject`) to get an isolated namespace.

**When to call:** whenever you need to prove where writes land — at
session start, or when a `recall` comes back empty and you suspect the
facts live in another namespace.

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

Fourteen MCP tools wrap the plan primitives from the
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

### `plan_done`

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

### `plan_show`

Fetch a plan with its full task list.

**Parameters:** `plan_id`, `ref?`.

---

### `plan_tasks`

List the tasks of a plan, flat.

**Parameters:** `plan_id`, `ref?`.

---

### `plan_move`

Move a plan and every task it contains from one branch (`ref`) to
another. Task ids, statuses, proofs, and the plan-meta envelope are
preserved bit-for-bit — only the ref changes.

**When to call:** promoting a sandboxed plan onto `main`, pulling
someone else's plan onto a feature branch to collaborate, or refiling
work after a branch-strategy change. Refuses when source and target are
the same ref, or when a plan with the same name already exists on the
target ref.

**Parameters:** `plan_id`, `target_ref`, `ref?` (source branch, default `main`).

---

### `plan_complete`

**Force-complete an entire plan in one shot** — marks every remaining
task done with a "force-complete" reason and closes the plan. Use
when remaining work is no longer relevant (scope cut, plan
superseded). For incremental progress, prefer `plan_done` per task.

**Parameters:** `plan_id`, `reason`, `ref?`.

---

### `plan_archive`

Set plan status to `archived`. Soft — task data is preserved.

**Parameters:** `plan_id`, `ref?`.

---

### `plan_link`

Record that a task, when done, **satisfies a task in another plan** — a
cross-plan dependency pointer (the substrate's `blocked_by` is within-plan
only). Advisory: it does not auto-close the target; completing the linked task
surfaces a reminder to close the other one.

**When to call:** work in one plan closes out a task tracked in another.

**Parameters:** `plan_id`, `task_id`, `target` (as `"plan/task"`), `ref?`.

---

### `plan_stale`

List **in-progress tasks idle for N days** (default 7) across active plans,
most-stale first. The drift guard `plan_start`'s warning defends against.

**When to call:** resuming work or auditing state to catch tasks left silently
in-progress. Complements `plan_next`, which only returns the next *pending*
task and never shows what's in progress.

**Parameters:** `days?` (default 7), `ref?`.

---

## Docs

### `docs_find`

Find registered canonical docs whose path/scope/answers/owner match a query
(omit `query` to list all). Returns each doc's path, status, scope, and what it
answers — a **pointer** to the canonical `.md` file, not its content.

**When to call:** before reading files or writing a new doc, to find where a
topic is already documented. Distinct from `prime`, which imports doc *content*
into `recall`.

**Parameters:** `query?`, `ref?`.

---

## Governance

These tools mediate writes that shouldn't be casual: deletion,
branch creation, and trust controls. They exist as MCP tools so
agents can introspect and act on the same surfaces a human reaches
for in Lens or via `ctx`.

### `forget`

Forget a path by writing a rollback commit. The data isn't physically
removed — its history is preserved in blame — but `get` and
`recall` will no longer surface it.

**Parameters:** `path`, `reason?`, `ref?`.

**When to call:** when a fact is wrong, expired, or the user
explicitly asks to forget it. Prefer `forget` over editing a fact in
place — the audit trail tells the next session "this used to be true,
here's why we stopped believing it."

---

### `branches`

List every branch with its current head commit id.

**Parameters:** none.

---

### `branch`

Create a new branch starting from `from` (default `"main"`).

**Parameters:** `name`, `from?`.

**When to call:** before doing speculative or breaking work. Branches
sandbox both code and memory — `remember` calls on a feature branch
stay there until the branch merges. See
[MEMORY_BRANCH_SCOPING.md](MEMORY_BRANCH_SCOPING.md) for the full
model.

---

### `merge`

Merge `source` into `target` (default `"main"`). Returns `{status:
"ok", commit_id}` on success, or `{status: "conflict", conflicts:
[...]}` if the 3-way merge can't proceed cleanly. Conflicting paths
are returned structurally so the caller can write the desired value
on `target` and re-attempt.

**Parameters:** `source`, `target?`, `description?`, `reasoning?`.

**When to call:** once a feature branch is ready to land back on the
trunk. Mirror of the `ctx merge` CLI command.

---

### `taint_list`

List active taints / quarantines / watches across the graph.

**Parameters:** `path_prefix?`, `effect?`.

---

### `taint_check`

Check whether `agent_id` may write to `path` at the given
`confidence`, given any active taints/quarantines. Returns
`can_write`, `effect` (the strongest blocking effect, if any), and
the matching taint id.

**Parameters:** `path`, `agent_id`, `confidence?`.

**When to call:** before any write you're not certain is allowed —
especially in multi-agent settings where another agent may have
fenced off a path.

---

### `taint_apply`

Apply a taint, quarantine, or watch to a path. `kind` selects the
variant: `taint` (with an `effect` of `warn`/`block`/`review`/
`isolate`/`advisory`), `quarantine` (with optional `authorized_agents`
whitelist), or `watch` (advisory tracking).

**Parameters:** `path`, `kind`, `reason`, plus `effect?`,
`authorized_agents?`, `min_confidence?`, `expires_at?`.

---

### `taint_remove`

Resolve (lift) an active taint, quarantine, or watch by id. The
record isn't deleted — it's marked resolved with a reason for audit.

**Parameters:** `taint_id`, `reason`.

---

## Reminders

Pull-based scheduling primitives. Create a reminder now; call
`remind_me` at any future checkpoint to retrieve what's due. Reminders
are ordered by priority then `due_at`. The `autonomous` flag controls
whether execution requires user approval first.

### `reminder_create`

Schedule a reminder for yourself or another agent.

**Parameters:** `title`, `instructions`, `due_at` (ISO 8601),
`priority?` (critical|high|medium|low|minimal, default medium),
`autonomous?` (default true), `schedule?` (once|interval|daily|weekly),
`commands?`, `refs?`, `tags?`.

---

### `remind_me`

Return all currently actionable reminders (`due` or
`awaiting_permission`), ordered by priority. Lazily promotes any
`pending` reminders whose `due_at` has passed. **Call this at session
start and after completing any task.**

**Parameters:** none.

---

### `reminder_list`

List reminders with optional filters.

**Parameters:** `status?`, `priority_at_most?`, `created_by?`,
`due_before?`, `ref_id?`, `tags?`.

---

### `reminder_get`

Retrieve a single reminder by id, including its full execution history.

**Parameters:** `id`.

---

### `reminder_snooze`

Defer a reminder to a later time without cancelling it.

**Parameters:** `id`, `until` (ISO 8601).

---

### `reminder_approve`

Approve a non-autonomous reminder for execution. Transitions
`awaiting_permission` → `due`.

**Parameters:** `id`, `approved_by?`.

---

### `reminder_cancel`

Cancel a reminder permanently. Use `reminder_snooze` to defer instead.

**Parameters:** `id`.

---

### `reminder_start`

Mark a reminder as in-progress. Opens a partial execution record.
Call just before acting; follow with `reminder_record` when done.

**Parameters:** `id`, `agent_id?`.

---

### `reminder_record`

Record the outcome of an execution attempt.
`result` is one of: `success` | `failed` | `deferred` | `snoozed` | `cancelled`.
On `success` with a repeating schedule, resets to `pending` with a new `due_at`.

**Parameters:** `id`, `result`, `notes?`, `task_id?`, `agent_id?`.

---

## Read primitives

Low-level read tools that mirror the engine's HTTP surface. Agents
generally reach for `recall` first; these are for cases where the
agent already knows the path or wants to introspect provenance.

### `get`

Read the JSON value stored at a path.

**Parameters:** `path`, `ref?`.

---

### `ls`

List every path under `prefix` on the given branch.

**Parameters:** `prefix?`, `ref?`.

---

### `search`

Full-text substring search across every stored value on the branch.

**Parameters:** `query`, `ref?`.

---

### `log`

Return the last N commits on a branch — newest first — with agent
id, intent category, description, confidence, and tags.

**Parameters:** `limit?`, `ref?`.

---

### `blame`

Return the full provenance chain for a path: every commit that
touched it, who wrote it, with what intent and confidence.

**Parameters:** `path`, `ref?`.

---

### `diff`

Compute the structural diff between two refs (branches or commits).

**Parameters:** `ref_a`, `ref_b`.

---

---

## Code intelligence tools

Code intelligence tools proxy read requests to an
[AgentStateDeveloper (ASD)](https://github.com/agentstatelabs/AgentStateDeveloper)
server. They require the hub to be started with at least one ASD repo
configured — either via `--asd-repo` (hub manages `asd-serve` processes) or
`--asd-url` (you run `asd-serve` yourself).

### Wiring ASD into the hub

**Option A — hub-managed process pool (recommended):**

```bash
ctxone-hub --http \
  --asd-repo myproject=/home/user/myproject/.asd-state.db \
  --asd-repo otherlib=/home/user/otherlib/.asd-state.db
```

The hub spawns one `asd-serve` process per repo on first use, kills it after
5 minutes of idle, and restarts it transparently on the next request.

**Option B — pre-running `asd-serve` (manual):**

```bash
asd-serve --db /home/user/myproject/.asd-state.db &
# Prints: listening on 127.0.0.1:4120

ctxone-hub --http --asd-url myproject=http://127.0.0.1:4120
```

Multiple `--asd-repo` / `--asd-url` flags can be mixed. The repo `name` you
supply becomes the value for the `repo` parameter in every code tool call.

---

### `code_repos`

List all ASD repos registered with this hub.

**Parameters:** _(none)_

**Returns:** JSON array `[{ name, url }]`.

```json
// response
[
  { "name": "myproject", "url": "http://127.0.0.1:54321" },
  { "name": "otherlib",  "url": "pool:otherlib" }
]
```

`url` is the resolved endpoint for pre-running repos; pool-managed repos show
`pool:<name>` until the process is first requested.

---

### `code_search`

Search code symbols by concept or keyword across name, signature, doc
comment, file path, and ledger summaries. Returns ranked results.

Use this for feature archaeology when you don't know the exact symbol name.

**Parameters:**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `query` | string | — | Search terms (concept, function name, effect label, …) |
| `repo` | string | first registered | Repo name from `code_repos`. Omit when only one repo is registered. |
| `kind` | string | — | Filter to `function`, `method`, `class`, `module`, `variable` |
| `language` | string | — | Filter to `python` or `typescript` |
| `limit` | number | 20 | Max results |

**Returns:** JSON array of matching symbol records with relevance scores.

```json
// request
{ "query": "charge card payment", "repo": "myproject", "limit": 5 }

// response
[
  {
    "qname": "payments.charge_card",
    "kind": "function",
    "file": "payments.py",
    "signature": "def charge_card(user_id: str, amount: float)",
    "score": 0.92
  }
]
```

---

### `code_read`

Read a symbol by qualified name. Returns the symbol, its declared and
transitive effects, and all ledger decisions — the full context needed to
reason about a code unit.

**Parameters:**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `qname` | string | — | Fully-qualified symbol name (e.g. `payments.charge_card`) |
| `repo` | string | first registered | Repo name |

**Returns:** `{ symbol, effects, ledger }`.

```json
// request
{ "qname": "payments.charge_card", "repo": "myproject" }

// response
{
  "symbol": {
    "qname": "payments.charge_card",
    "kind": "function",
    "file": "payments.py",
    "signature": "def charge_card(user_id: str, amount: float)"
  },
  "effects": {
    "declared": [
      { "effect": "io.db.write", "note": "INSERT INTO charges" },
      { "effect": "log" },
      { "effect": "throw" }
    ],
    "transitive": [],
    "verification": { "status": "unverified" }
  },
  "ledger": [
    {
      "kind": "hazard",
      "summary": "boundary at 10000 is undocumented",
      "tags": ["approved", "approved-by:alice@example.com"]
    }
  ]
}
```

---

### `callers_of`

List symbols that call the given symbol (inbound call edges).

**Parameters:**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `qname` | string | — | Target symbol |
| `repo` | string | first registered | Repo name |

**Returns:** JSON array of calling symbol records.

```json
// request
{ "qname": "payments.charge_card", "repo": "myproject" }

// response
[
  { "qname": "driver.main", "kind": "function", "file": "_driver.py" }
]
```

---

### `callees_of`

List symbols called by the given symbol (outbound call edges).

**Parameters:**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `qname` | string | — | Target symbol |
| `repo` | string | first registered | Repo name |

**Returns:** JSON array of callee symbol records.

```json
// request
{ "qname": "driver.main", "repo": "myproject" }

// response
[
  { "qname": "payments.charge_card", "kind": "function", "file": "payments.py" },
  { "qname": "payments.get_balance", "kind": "function", "file": "payments.py" }
]
```

---

### `get_active_repo`

Return the active ASD repo for this session (set via `set_active_repo`),
or `null` if none is set. Also returns the list of registered repo names
so the agent can pick one.

**Parameters:** none.

**Returns:** `{ "active_repo": <name|null>, "known_repos": [ … ] }`.

---

### `set_active_repo`

Set the active ASD repo for this session. Subsequent code tools
(`code_search`, `code_read`, `callers_of`, `callees_of`) default to this
repo when their `repo` parameter is omitted — so you don't repeat `repo`
on every call in a single-repo session.

**When to call:** at the start of a session focused on one registered
repo. Errors if the repo isn't registered; pass an empty string to clear.

**Parameters:** `repo` (registered repo name; empty string clears).

---

## See also

- [HTTP_API.md](HTTP_API.md) — same logic exposed over REST
- [INTEGRATIONS.md](INTEGRATIONS.md) — how to wire these tools into specific AI clients
- [ASD_INTEGRATION.md](ASD_INTEGRATION.md) — full guide to the ASD code intelligence integration
- [ARCHITECTURE.md](ARCHITECTURE.md) — the underlying graph model
- [AGENTS.md](AGENTS.md) — guidance on when to reach for plans vs. inline work

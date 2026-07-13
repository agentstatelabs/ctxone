# Architecture — The Mental Model

This is the conceptual walkthrough. For API details, see the command reference.
For strategy, see [VISION.md](VISION.md).

## The one-sentence pitch

CTXone is a **versioned, searchable, structured memory store** that gives AI
agents persistent context without the token cost of carrying that context on
every turn.

## Why "O(log n)" beats "flat memory files"

A flat memory file grows linearly with your project knowledge. Every message to
the agent ships the whole file — useful facts, dead facts, and everything in
between. More knowledge makes the agent smarter but every turn costs more.

```
turn 1:  [whole memory file] + [message]  → cost O(n)
turn 2:  [whole memory file] + [message]  → cost O(n)
turn 3:  [whole memory file] + [message]  → cost O(n)
...
```

CTXone inverts this. The graph is stored once, on disk. Every recall fetches
**only the facts relevant to the current question**, plus a small set of
always-included "pinned" facts. Adding more knowledge to the graph doesn't
increase per-turn cost.

```
turn 1:  [pinned + relevant 3 facts] + [message]  → cost O(log n)
turn 2:  [pinned + relevant 5 facts] + [message]  → cost O(log n)
turn 3:  [pinned + relevant 2 facts] + [message]  → cost O(log n)
...
```

The engine is [AgentStateGraph](https://github.com/agentstatelabs/agentstategraph) —
a branching, versioned state store with intent metadata. CTXone layers a
memory-oriented API on top.

## The four kinds of memory

Every fact in the graph falls into one of these buckets:

### 1. Pinned memory (`/memory/pinned/<source>/<slug>`)

Always included in every recall response, regardless of topic. Use for
critical context the agent must always see: project conventions, licensing,
current status, team norms.

**Added via:** `ctx prime <file.md> --pin`

**Storage:** Structured `{title, body}` pairs. Grouped by source so you can
re-prime from the same markdown file idempotently.

### 2. Primed memory (`/memory/primed/<source>/<slug>`)

Searchable like normal facts but grouped into a named source. Use for
reference material you want the agent to find when relevant but not pay for
on every call: API docs, design notes, historical decisions.

**Added via:** `ctx prime <file.md>` (no `--pin`)

**Storage:** Same as pinned but under `/memory/primed/*`.

### 3. Free-form facts (`/memory/<context>/<id>`)

Individual facts added incrementally. Use for accumulating project knowledge
as you work: decisions, bug fixes, preferences, status updates.

**Added via:** `ctx remember "fact" --context <category>`

**Storage:** String values under `/memory/<context>/<timestamp>`.

### 4. Sessions (`/sessions/<session_id>/{summary,decisions,details}`)

End-of-session commits capturing what was learned in a working session.
Enables the "close a session, open a new one, context preserved" workflow.

**Added via:** MCP `summarize_session` tool (agents call this automatically)

## How `recall` ranks results

When you (or an agent) runs `recall "topic"`, here's what the Hub does:

1. **Fetch all pinned memories.** These go into the response first, up to half
   the token budget. No matter what you searched for, pinned content is always
   included.

2. **Tokenize the query.** Split on whitespace, drop stopwords and short
   tokens. `"licensing decisions"` becomes `["licensing", "decisions"]`.

3. **Run a full-phrase search** for the exact query string. Hits get marked
   `full_match: true` and win ties.

4. **Run a per-token search** for each query token. Aggregate by path: each
   path's score is the number of tokens that matched, weighted so full-phrase
   hits outrank same-count token hits.

5. **Deduplicate** — don't repeat a pinned section if its title or body also
   matches the topic search.

6. **Budget-cap** — add results in rank order until the remaining token budget
   is spent. Half for pinned, half for topic matches.

7. **Return** the combined list plus metadata: pinned count, topic match count,
   tokens sent, flat-memory baseline, savings ratio.

## How savings are measured

Every response carries three numbers:

- `ctx_tokens_sent`: how many tokens this specific response used
- `ctx_tokens_estimated_flat`: how many tokens would have been sent if the
  whole graph were loaded instead (the flat-memory baseline)
- `ctx_savings_ratio`: `flat / sent`

The baseline is computed by serializing the entire graph to JSON and dividing
the character count by 4 (a rough tokens-per-char estimate). It's cached
between writes and only recomputed when the graph changes.

See [TOKEN_SAVINGS.md](TOKEN_SAVINGS.md) for the full derivation and how to
read the numbers.

## Branches (multi-context memory)

The graph supports git-style branches. Every read and write takes an optional
`--branch` parameter. Common patterns:

- **Main branch** — your shared project memory
- **Experiment branches** — try priming a large doc, see if recall improves,
  merge or discard
- **Per-session branches** — give a one-off session its own memory namespace
  without polluting main
- **Per-agent branches** — each agent writes to its own branch, with
  periodic merges

```bash
ctx branch experiment
ctx --branch experiment prime ./big-proposal.md --pin
ctx --branch experiment recall "architecture"  # see if it helps
ctx diff main experiment                         # compare
```

## Provenance (blame)

Every write carries intent metadata: an agent ID, a category (Observe,
Checkpoint, Refine, etc.), a description, and a confidence score. `ctx blame
<path>` returns the full chain of commits that touched a path, so you can
answer "where did this fact come from?" and "when was it last updated?"

This turns the memory graph from a black box into an auditable,
accountable ledger.

## Token tracking is built in, not bolted on

The Hub maintains session-wide counters for tokens sent and tokens saved.
Every `recall` updates them. `ctx stats` and `GET /api/stats/tokens` return
the cumulative totals — making the savings claim measurable, not a vibes
improvement.

## The full stack

```
┌─────────────────────────────────────────────────────────┐
│  Your AI tools                                          │
│  ┌──────────────┐  ┌──────────┐  ┌────────┐  ┌───────┐  │
│  │ Claude Code  │  │  Cursor  │  │ VSCode │  │ Codex │  │
│  └──────┬───────┘  └────┬─────┘  └────┬───┘  └───┬───┘  │
│         │               │              │          │      │
│         └────── MCP over HTTP (POST /mcp) ───────────┘      │
│                         │                                  │
└─────────────────────────┼──────────────────────────────────┘
                          │
                          ▼
    ┌───────────────────────────────────────────────────┐
    │      CTXone Hub — one process (ctxone-hub)        │
    │   MCP at /mcp: remember / recall / prime / ...    │
    │   HTTP API: /api/memory/*, /api/state/*           │
    │   Lens web UI (--lens) · token tracker            │
    └──────────────────────┬────────────────────────────┘
                           │
                           ▼
    ┌───────────────────────────────────────────────────┐
    │       CTXone Engine (AgentStateGraph)             │
    │   Versioned state store, branches, blame,         │
    │   intent metadata, confidence scoring             │
    └──────────────────────┬────────────────────────────┘
                           │
                           ▼
                  ┌─────────────────┐
                  │  SQLite /       │
                  │  Postgres /     │
                  │  in-memory      │
                  └─────────────────┘
```

The same process also serves two other surfaces over the same HTTP listener:

- **`ctx` CLI** — scripting and inspection (`remember`, `recall`, `search`,
  `log`, `diff`, `tail`, ...)
- **CTXone Lens** — SvelteKit web UI for browsing the graph visually (`--lens`)

All surfaces read and write the same graph — there's no parallel memory.

### The unified daemon is the standard setup

`ctxone-hub --http` — the **[unified daemon](design/UNIFIED_HUB.md)** — serves
MCP at `POST /mcp` alongside the REST API and (with `--lens`) the web UI, all
from **one process** that owns the db. Agents connect by URL, so the diagram
above is literally true: one Hub, every surface. `ctx init` defaults to this
(`--transport http`), and `ctx service install` runs it as an always-on
login/boot daemon. This is the recommended setup for everyone — see
**[DEPLOYMENT.md](DEPLOYMENT.md)**.

**The stdio fallback.** MCP can also be served per-client over stdio
(`ctx init --transport stdio`): each agent *spawns its own* `ctxone-hub` child
that opens the db directly. Because a hub takes an exclusive lock on its db file
(see [DATA_SAFETY.md](DATA_SAFETY.md)), an agent's stdio hub and a `--http --lens`
hub **cannot both own the same db at once** — whoever starts first wins the lock,
which is exactly the startup-order race the daemon removes. Use stdio only when a
client can't speak HTTP MCP or you deliberately want each agent on its own db;
see [DEPLOYMENT.md](DEPLOYMENT.md) for the topology matrix.

## What the Hub is not

- **Not a vector store.** Retrieval is structural path + substring search,
  not similarity. This keeps it predictable and blame-able — you know exactly
  why each result came back.
- **Not a message queue.** The graph is state, not events. Use a log/stream
  for audit trails beyond commit-level provenance.
- **Not a Claude-specific tool.** MCP is the default surface, but the HTTP
  API works for any client. The `ctx` CLI proves this.

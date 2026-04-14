# agentstategraph-memory — MCP Server Design

## Overview

A thin MCP server layer over the existing AgentStateGraph 26 tools that provides
higher-level memory operations optimized for the agent memory use case.

Same binary, same storage, friendlier API surface. No new core APIs needed —
everything maps to existing operations.

## Operations

### remember(fact, importance, context)

Store a fact in agent memory with structured metadata.

**Maps to:** `agentstategraph_set` with confidence + tags + intent

```json
{
  "tool": "remember",
  "params": {
    "fact": "Craig prefers BSL 1.1 for all projects",
    "importance": "high",
    "context": "licensing",
    "tags": ["preference", "licensing"]
  }
}
```

Internally:
- Path: `/memory/facts/{generated-id}` or `/memory/preferences/licensing`
- Intent category: Observe (for facts) or Checkpoint (for decisions)
- Confidence: mapped from importance (high=0.95, medium=0.7, low=0.4)
- Tags: passed through for queryability

### recall(topic, budget)

Retrieve relevant memories for a topic, respecting a token budget.

**Maps to:** `agentstategraph_search` + `agentstategraph_query` with limit

```json
{
  "tool": "recall",
  "params": {
    "topic": "current project status",
    "budget": 1500
  }
}
```

Internally:
- search_values(query=topic) for value matches
- query(reasoning_contains=topic) for intent/reasoning matches
- Combine, deduplicate, truncate to budget
- Return structured results with path, value, confidence, timestamp

### context(project)

Load the full context tree for a specific project or domain.

**Maps to:** `agentstategraph_get_tree` on the project branch

```json
{
  "tool": "context",
  "params": {
    "project": "agentstategraph"
  }
}
```

Internally:
- get_tree(ref="main", prefix="/projects/{project}")
- Returns nested JSON with all state under that project

### summarize_session(key_points)

End-of-session commit capturing what was learned and decided.

**Maps to:** `agentstategraph_set` at summary path + Checkpoint intent

```json
{
  "tool": "summarize_session",
  "params": {
    "session_id": "2026-04-13-afternoon",
    "key_points": [
      "Shipped Postgres backend with multi-tenant support",
      "Built auth middleware with API key management",
      "Discussed context anxiety concept and agent memory use case"
    ],
    "decisions": [
      "SaaS model: try-before-you-buy, not the business",
      "Self-hosted is the compliance path",
      "Agent memory is the highest-leverage next product"
    ]
  }
}
```

Internally:
- set("/sessions/{session_id}/summary", condensed_text, Checkpoint)
- set("/sessions/{session_id}/decisions", decisions_array, Checkpoint)
- set("/sessions/{session_id}/details", full_points, Observe)

### what_changed_since(date)

See what's new since the last session.

**Maps to:** `agentstategraph_query` with date filter

```json
{
  "tool": "what_changed_since",
  "params": {
    "since": "2026-04-12T00:00:00Z"
  }
}
```

### why_did_we(decision)

Trace the reasoning behind a past decision.

**Maps to:** `agentstategraph_blame` on the decision path

```json
{
  "tool": "why_did_we",
  "params": {
    "decision": "use BSL 1.1"
  }
}
```

Internally:
- search_values(query="BSL 1.1") to find the path
- blame(path) to get the full provenance chain

## Storage Schema for Memory

```
/memory/
  preferences/          # User preferences (high confidence, rarely change)
    licensing           → "BSL 1.1"
    deployment          → "self-hosted preferred"
    coding/style        → "Rust, clean warnings"
  
  projects/
    agentstategraph/
      status            → "0.3.5-beta.2, pre-launch"
      recent_work       → ["Postgres backend", "auth middleware", "explorer"]
      decisions/        # Key decisions with blame trails
        license         → "BSL 1.1"
        saas_model      → "try-before-you-buy on-ramp"
        no_clustering   → "not needed for launch"
    threadweaver/
      status            → "0.3.5-beta.2, tools toggle shipped"
      recent_work       → ["markdown rendering", "tool detection"]
  
  contacts/
    enterprise/
      banking           → {name, company, status, last_contact}
      medical           → {name, company, status, last_contact}
    partners/
      european_gitops   → {relationship, status, pitch_angle}
  
  sessions/
    2026-04-13/
      summary           → "Shipped Postgres, auth, explorer. Discussed memory."
      decisions         → ["SaaS as on-ramp", "agent memory is top priority"]
      details           → [full list of work items]
    2026-04-12/
      summary           → "Naming hygiene, BSL license, landing pages."
      decisions         → ["BSL 1.1", "blue/teal for ASG site"]

  ideas/
    context_anxiety     → {description, marketing_angles, status}
    mobile_app          → {architecture, revenue_model, timeline}
```

## Session Lifecycle

### Session Start (agent priming)
1. `recall("current work", budget=1000)` → recent session summaries + project status
2. `context(project)` if user specifies a project → full project tree
3. Agent has ~1,000 tokens of highly relevant context. Ready to work.

### During Session
4. Agent encounters new information → `remember(fact, importance, context)`
5. User makes a decision → `remember(decision, "high", "decisions")`
6. Agent needs historical context → `recall(topic)` or `why_did_we(decision)`

### Session End
7. `summarize_session(key_points, decisions)` → commits the session's learnings
8. Next session will see this summary in step 1. Circle complete.

## Implementation Approach

Two options:

### Option A: Separate MCP server binary
- New crate: `crates/agentstategraph-memory/`
- Depends on `agentstategraph` (same repo)
- 6 tools instead of 26 (simpler for memory-only users)
- Separate MCP config entry

### Option B: Tool prefix on existing server
- Add `memory_*` tools to the existing `agentstategraph-mcp` binary
- `--memory` flag enables the memory tools alongside the 26 existing tools
- Single binary, single config entry
- Users who want both memory and raw tools get them together

**Recommendation:** Option B for launch (faster, no new binary), Option A for
the standalone "memory for any AI" product (cleaner, focused).

## Token Budget Enforcement

The `recall` operation respects a token budget by:
1. Running the search query
2. Estimating token count per result (~4 chars per token)
3. Returning results until budget is reached
4. Highest-confidence results first (sorted by confidence desc)

This means the agent can say "give me the most important memories that fit in
1,500 tokens" and get a maximally relevant context window.

# Token Economics — The Math Behind AgentStateGraph Memory

> **Calibration note.** The ratios on this page (60× per session, etc.) are
> a theoretical ceiling derived from a specific model of "flat memory
> dumped into context every turn." They are not the number we promise on
> the landing page. The **live** `ctx_savings_ratio` the Hub computes on
> every recall starts around **5×** on a fresh graph and climbs as the
> graph matures — that 5× is what we quote publicly. The 60× ceiling is
> what the math allows under optimistic conditions; real users sit
> somewhere between 5× and that ceiling depending on graph size and how
> scoped their recalls are. For the measurement details see
> [TOKEN_SAVINGS.md](TOKEN_SAVINGS.md).

## The Current Model (Flat Memory)

Memory file loaded into context on every turn:

```
Memory file:        3,000 tokens (modest — grows quickly)
Conversation:       20 turns average
Token cost/session: 3,000 × 20 = 60,000 tokens on memory alone
```

At 10,000 tokens (realistic for active project):
```
10,000 × 20 = 200,000 tokens per conversation on memory
```

This is JUST memory overhead — on top of the actual conversation tokens.

## The AgentStateGraph Model (Selective Retrieval)

Agent queries for what's relevant instead of loading everything:

```
Session start: 3 MCP tool calls
  search_values("current project")               → 150 tokens
  get_tree("/projects/agentstategraph/status")    → 200 tokens
  query(category="Checkpoint", confidence>0.8)    → 300 tokens

Total context loaded: ~650 tokens (once, not per-turn)
Mid-conversation queries: maybe 2 × 200 tokens = 400 tokens
Total memory tokens per session: ~1,050 tokens
```

## The Comparison

| | Flat Memory | AgentStateGraph | Savings |
|---|---|---|---|
| Per session | 60,000 tokens | ~1,000 tokens | **60x** |
| Per day (10 sessions) | 600,000 tokens | ~10,000 tokens | 60x |
| Per month | 18M tokens | 300k tokens | 60x |

## Enterprise ROI

### Mid-sized company: 10 agents, 50 conversations/day

**Without AgentStateGraph:**
- 10 × 50 × 60,000 = 30M tokens/day
- At $3/M tokens (Claude Sonnet): $90/day = **$2,700/month**
- Memory overhead alone

**With AgentStateGraph:**
- 10 × 50 × 1,000 = 500k tokens/day
- At $3/M tokens: $1.50/day = **$45/month**
- Savings: **$2,655/month** = **$31,860/year**

### Large enterprise: 50 agents, 100 conversations/day

**Without AgentStateGraph:**
- 50 × 100 × 60,000 = 300M tokens/day
- At $3/M tokens: $900/day = **$27,000/month**

**With AgentStateGraph:**
- 50 × 100 × 1,000 = 5M tokens/day
- At $3/M tokens: $15/day = **$450/month**
- Savings: **$26,550/month** = **$318,600/year**

### The pitch

"AgentStateGraph Enterprise costs $50-250k/year. It saves $300k/year on token
costs alone — before counting the value of consistent session quality, full
accountability, and transparent agent state. It pays for itself."

## Why This Is Structural, Not Incremental

Flat memory scales linearly with knowledge: more facts = more tokens per turn.
The problem gets WORSE the more useful the agent becomes.

AgentStateGraph memory scales logarithmically: more facts in the graph, but
each session only loads what's relevant. More knowledge = same token cost.
The agent gets smarter WITHOUT getting more expensive.

This is the difference between O(n) and O(log n) scaling on memory costs.

## Additional Token Savings

### Hierarchical summarization

```
/sessions/2026-04-13/summary    → 50 tokens  (always loaded)
/sessions/2026-04-13/decisions  → 100 tokens (loaded if relevant)
/sessions/2026-04-13/details    → 2000 tokens (only on drill-down)
```

Agent loads the 50-token summary. If it needs detail, it drills into the
100-token decisions. Only if it needs the full context does it load 2000 tokens.
Tree structure = naturally hierarchical, naturally token-efficient.

### Confidence-based filtering

```
query(confidence_min=0.8)  → only load high-confidence facts
query(confidence_min=0.5)  → include tentative observations (when exploring)
```

Importance is encoded at write time via confidence scores. No embedding
similarity search, no vector distance heuristics — structured metadata
captured at the moment of knowledge creation.

### Token-budgeted loading

Agent receives a budget: "prime from AgentStateGraph, stay under 1,500 tokens."
Query system supports limits. Agent controls exactly how much memory it loads
based on what the context window can afford.

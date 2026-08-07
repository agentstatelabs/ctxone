# Cost Savings — How CTXone Pays for Itself

CTXone earns its keep on **cost savings alone** — before you count any gain
in agent quality or developer time. Every agent you run has a token bill, and
most of that bill is spent re-establishing context the agent should already
have. CTXone drives that recurring cost toward zero.

This doc is the money view. For *how* the token ratio is measured, see
[TOKEN_SAVINGS.md](TOKEN_SAVINGS.md). For the underlying economics, see
[TOKEN_ECONOMICS.md](TOKEN_ECONOMICS.md).

## Where the money goes without CTXone

A coding agent's cost is dominated by input tokens, and input tokens are
dominated by **repeated context**:

- **Cold-start cost.** Every new agent session begins by re-reading the
  README, the architecture docs, recent diffs, and half a dozen source files
  just to orient. That's thousands of tokens spent before the first useful
  action — paid again on the *next* session, and the one after that.
- **Context you already paid for.** The same facts ("we use BSL-1.1", "the
  hub owns the db", "auth lives in `session.rs`") get re-derived from scratch
  in session after session because nothing carries them forward.
- **Runaway sessions.** A session that loses the thread keeps pulling more
  files into context to compensate, and the token bill grows with it — often
  well past the point where the agent is still making progress.

CTXone attacks each of these directly.

## 1. Cheaper agent startup

Spinning up a new agent is the single most repeated cost in an agent fleet.
Instead of re-reading the repo, a CTXone-backed agent calls `recall` (or
`prime`) once and gets exactly the pinned context and topic-relevant memories
it needs, inside a token budget.

```bash
ctx recall "how does auth work"   # ~30 tokens sent vs re-reading 2k+ of source
```

The cold-start that used to cost thousands of input tokens becomes a single
budgeted recall. Multiply that by every session, every agent, every day —
that's the bulk of the savings.

## 2. Shared memory across agents and sessions

One hub, one memory graph. When any agent stores a fact, every other agent —
now and in the future — recalls it for near-free instead of rediscovering it.

- A decision made in Monday's session is available to Tuesday's agent without
  re-analysis.
- Two agents working the same repo share one namespace, so neither pays to
  learn what the other already learned.
- Context is written **once** and read **many** times, at recall cost rather
  than rediscovery cost.

The economic shift is from *O(sessions)* rediscovery to *O(1)* discovery plus
cheap recalls. See [MEMORY_BRANCH_SCOPING.md](MEMORY_BRANCH_SCOPING.md) for how
namespaces keep each repo's memory isolated.

## 3. Shared context across code spaces

CTXone's memory isn't tied to a single checkout. A `.ctxproject` marker binds
a namespace to the repo, so every code space — a laptop, a CI runner, a cloud
dev environment, a teammate's clone — resolves to the **same** memory graph.

You pay to establish a project's context once; every code space that touches
that repo inherits it for free. No per-environment re-onboarding cost.

## 4. Token savings, tracked as dollars

Savings you can't see are savings you can't defend. Every recall reports
tokens sent versus the flat-memory baseline, and the hub accumulates the
running total per session:

```
recall "licensing"    →  2 matches, 34 tokens sent vs 451 flat (13.0x savings)
Cumulative this session:  98 tokens sent, 1706 tokens saved, 18.4x overall
```

Because the hub records token usage per model, those saved tokens convert
directly to a dollar figure at current model pricing — the Lens UI surfaces
cost-per-feature and top-saver views built on exactly this data. The savings
stop being a hand-wave and become a line item.

## 5. Catching over-budget, token-burning sessions

The most expensive session is the one that's burning tokens without making
progress. Because CTXone tracks per-session usage, over-budget sessions become
**visible** instead of silently draining the bill:

- Sessions that cross a token budget surface in the usage views.
- A session whose token spend keeps climbing while useful output flattens is
  the signal to stop, checkpoint, and restart clean — rather than let it run.
- You spend where there's return and cut where there isn't.

This turns token spend from an after-the-fact invoice surprise into something
you steer in real time.

## 6. Restart clean — new session from old, with provenance

When a session goes over budget, you don't throw the value away. CTXone lets
you seed a **fresh** session from a prior one, carrying forward only what
matters:

- **Provenance** — the new session knows which decisions and facts came from
  the old one and why, so nothing is silently lost.
- **User importance** — high-importance memories and pinned context come
  along; low-value chatter does not.

The result is a cheap, focused session that inherits the expensive thinking of
the old one without inheriting its bloated context window. You pay once for the
hard reasoning and reuse it at recall cost.

## The bottom line

Add it up and the pattern is the same in every line item: **stop paying
repeatedly for context you've already established.**

- Startup context: paid once, recalled cheaply thereafter.
- Shared memory + code spaces: discovered once, read many times.
- Tracked savings: measured in tokens and dollars, not vibes.
- Over-budget sessions: caught early and restarted clean with provenance.

For a fleet of agents running all day, the recurring cost CTXone removes is
larger than what CTXone costs to run — which is the whole point. It pays for
itself on cost savings alone; everything else (better, more grounded agents —
see [AGENT_STRUCTURE.md](AGENT_STRUCTURE.md)) is on top.

# Agent Structure — How CTXone Makes Agents Perform Better

A capable model with no structure is a fast way to burn tokens on the wrong
work. CTXone gives agents **structure**: a durable place to record what the
work is, what's already known, and what actually happened. Structured agents
start on track, stay on track, and hallucinate less — because they're
reasoning over recorded facts instead of guessing.

This doc covers the structural scaffolding CTXone provides. For the cost side
of the same story, see [COST_SAVINGS.md](COST_SAVINGS.md). For how memory is
scoped, see [MEMORY_BRANCH_SCOPING.md](MEMORY_BRANCH_SCOPING.md).

## Why unstructured agents drift

Left to themselves, agents fail in predictable ways:

- **They start cold.** With no shared statement of the goal, an agent invents
  its own interpretation of the task and works to the wrong target.
- **They forget.** Decisions made earlier — or by another agent — aren't
  visible, so they get re-litigated or contradicted.
- **They hallucinate.** With no grounded record of what the code does or what
  was decided, the model fills gaps with plausible-sounding fiction.

Every one of these is a *missing structure* problem, not a *smarter model*
problem. CTXone supplies the missing structure.

## 1. Plans and tasks — a shared statement of the work

Plans give the work an explicit, durable shape that any agent can read before
touching code:

- A **plan** captures the goal and its breakdown so the agent starts from the
  actual objective, not a guess at it.
- **Tasks** track discrete units of work with status, so an agent (or the next
  agent) knows what's done, what's in progress, and what's next.
- Because plans live in the hub, a handoff between agents — or a restart of a
  blown session — resumes against the same plan instead of starting over.

An agent that opens a plan before it starts is an agent that starts *on
track*. See the plan commands in [CLI_REFERENCE.md](CLI_REFERENCE.md) and the
MCP `plan_*` tools in [MCP_TOOLS.md](MCP_TOOLS.md).

## 2. Branch-scoped and pinned memories — the right context, always

Structure isn't just the task list; it's the context the agent reasons over.

- **Branch scoping** keeps each branch's memory separate, so an agent working
  a feature branch sees that branch's decisions and not unrelated noise from
  elsewhere. Context matches the work.
- **Pinned memories** are the always-included facts — licensing, architecture
  invariants, "the hub owns the db" — that every recall returns first. They're
  the guardrails that keep an agent from wandering off the established rules of
  the project.

Together they mean the agent's working context is *curated*, not accidental:
the critical facts are always present, and irrelevant ones stay out of the
window. That's the single biggest lever against hallucination — the model has
the real answer in front of it, so it doesn't invent one.

## 3. Recorded git commits and MRs — grounding in what actually happened

CTXone captures the project's real history so agents reason over facts, not
assumptions:

- **Recorded commits** give an agent an accurate account of what changed, when,
  and why — so "what's the current state of X" is answered from history rather
  than guessed.
- **Merge requests / PRs** tie decisions to the change that implemented them,
  preserving the *why* behind the code.
- Linking memory to commits and MRs gives every decision **provenance**: an
  agent can trace a fact back to the change that established it and trust it.

An agent grounded in recorded history stops hallucinating about the state of
the codebase, because the state is written down.

## 4. Provenance and importance — trustable context

Structure is only useful if the agent can trust it. CTXone attaches:

- **Provenance** — where a fact came from (which session, commit, or MR), so
  the agent can weigh it and a human can audit it.
- **Importance** — user-assigned weight, so recall surfaces the facts that
  matter most within a token budget instead of drowning signal in chatter.

This is what lets a recall return *the right few facts* rather than everything,
and lets a fresh session inherit the important, well-sourced context from a
prior one without the noise.

## How the structure compounds

The pieces reinforce each other:

1. A **plan** tells the agent what the work is.
2. **Pinned + branch-scoped memory** tells it the rules and the relevant
   context.
3. **Recorded commits and MRs** tell it what actually happened.
4. **Provenance and importance** let it trust and rank all of the above.

An agent standing on that scaffolding starts on track (it read the plan and the
pins), stays on track (its context is scoped and curated), and hallucinates
less (it reasons over recorded, sourced facts). The model does the thinking;
CTXone makes sure it's thinking about the right thing.

## Related

- [COST_SAVINGS.md](COST_SAVINGS.md) — the cost side: the same structure that
  grounds agents also stops you paying to re-establish it every session.
- [ARCHITECTURE.md](ARCHITECTURE.md) — how recall, priming, and the graph work.
- [AGENTS.md](AGENTS.md) — how to teach your AI tools to use CTXone.

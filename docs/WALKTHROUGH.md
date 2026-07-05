# CTXone Walkthrough — from install to daily use

The "sit down and actually use it" guide: the startup sequence, the daily
loop, what happens under the covers, how CTXone plugs into your LLM, and how
it works alongside AgentStateDeveloper. For the 5-minute version see
[QUICKSTART.md](QUICKSTART.md); for a flat command reference see
[FEATURES.md](FEATURES.md).

The one-sentence mental model: **CTXone is persistent, searchable,
accountable memory for AI agents — recall ranks the *relevant* context into
a token budget instead of re-loading everything, and every fact carries
provenance so you can trace why a decision was made.**

---

## 1. Startup sequence

```bash
# Install (macOS/Linux)
brew install ctxone/tap/ctxone
# or: curl -sSL .../install.sh | sh   |   docker run ... ghcr.io/ctxone/ctxone

# Start the Hub (memory server)
ctx serve --http --lens        # Hub + REST API + Lens UI on :3001

# Wire it into your agents
ctx init                       # detect + configure your AI tools (MCP)
ctx agents install             # write & pin the AGENTS.md guidance
ctx skill                      # install the Agent Skill (WHEN to use CTXone)
```

Or let the agent do it — run `ctx bootstrap` and paste the block into Claude
Code / Cursor / Codex. It installs, primes CTXone, and offers ASD too.

---

## 2. What just happened under the covers

- **The Hub** is a local server holding your memory graph in SQLite
  (`~/.ctxone/memory.db` by default). It speaks **MCP** (for agents) and,
  with `--http`, a **REST API** (for the Lens UI and scripts).
- **The memory graph** stores facts, decisions, and session summaries under
  content-addressed paths, with full history — every write is a commit you
  can `log`, `blame`, and `diff`.
- **Recall is ranked, not flat.** Instead of pasting your whole memory into
  every prompt, `recall` scores candidates against the topic and fills a
  token budget with the most relevant — pinned memories always included.
  See [ARCHITECTURE.md](ARCHITECTURE.md) for why this is O(log n).
- **Durability is on by default** — startup + periodic snapshots, a PID
  lockfile so a second hub can't corrupt the db, and an inode-drift
  watchdog. See [DATA_SAFETY.md](DATA_SAFETY.md).

---

## 3. The daily loop

**a. Remember the things worth keeping** — decisions, constraints, gotchas:
```bash
ctx remember "We use BSL-1.1 for all projects" --importance high --context licensing
```

**b. Recall on a cold start** instead of re-explaining the project:
```bash
ctx recall "licensing decisions"
ctx context myproject          # load the full project context
```

**c. Track multi-step work as a plan** that survives across sessions:
```bash
ctx plan new my-feature
ctx plan add my-feature "Wire up new endpoint"
ctx plan next my-feature                       # what should I do next?
ctx plan done my-feature t-001 --proof commit:abc1234
```

**d. Sandbox speculative work on a branch** — memory follows the branch:
```bash
ctx --branch feature/x remember "API renamed from foo to bar"
ctx diff main feature/x
ctx merge feature/x --into main
```

**e. Check the payoff:**
```bash
ctx status        # Hub health + connection
ctx stats         # token savings vs flat memory loading
```

**Before reversing a settled decision**, trace its provenance first:
```bash
ctx why-did-we "SQLite"        # who decided, when, and why
```

---

## 4. How CTXone plugs into your LLM

Three layers, same shape as its sibling:

1. **MCP tools** (`ctx init`). The agent can call `remember`, `recall`,
   plans, taint, `why_did_we`, etc. as structured tools — the capability
   layer. 42 tools in total.
2. **Always-on guidance** (`ctx agents install`). Writes `AGENTS.md` and
   primes it as pinned memory so the agent knows the tools exist and the
   house rules for using them.
3. **Agent Skill** (`ctx skill`). A `SKILL.md` that teaches the agent *when*
   to record a decision, open a plan, or recall context — so memory-keeping
   is triggered behavior, not an afterthought.

**Automatic capture.** Wire the Claude Code / Codex Stop hook to
`ctx capture-turn` and CTXone extracts memories + token usage from each turn
with no manual `remember` calls. `ctx ingest-session` backfills from
existing transcripts.

---

## 5. What to expect over time

- **Token savings grow with your memory.** The bigger and flatter your
  "just paste everything" baseline would have been, the more ranked recall
  saves. `ctx stats` makes it provable. See [TOKEN_SAVINGS.md](TOKEN_SAVINGS.md).
- **Memory stays accountable.** Every fact has a blame chain; nothing is an
  anonymous blob. Reversing a decision is a deliberate act, not an accident.
- **Plans don't rot.** Because a plan is a live object the agent updates with
  proof, it stays anchored to what actually shipped.

---

## 6. Using CTXone with AgentStateDeveloper (the suite)

CTXone is the **shared team memory** half. **[AgentStateDeveloper](https://github.com/agentstatelabs/AgentStateDeveloper)**
is the **per-developer code-context** half. They're built to pair — see
[ASD_INTEGRATION.md](ASD_INTEGRATION.md) for the deep version:

- **ASD** answers "what does *this code* do, and what will *this change*
  break?" — impact, invariants, effects, call graph, checked into each repo.
- **CTXone** answers "what has the *team* decided, and what are we doing?" —
  durable memory, plans, and decisions that travel across people and repos.

The joint loop: use ASD for the code specifics of a change, and record the
durable decision into CTXone so the whole team inherits it. ASD's
`task-close` even tags its proof entries with CTX plan/task provenance, so a
change closed in code lines up with the plan it belongs to.

When both are installed:

- `ctx skill` (or `asd skill`) also installs a **combined suite skill** that
  teaches the agent the joint workflow.
- `ctx bootstrap` offers to set up ASD too (and vice-versa).
- Installing either fires a one-time, dismissable nudge to add the other
  (suppress with `--no-nudge` / `CTX_NO_SUGGEST=1`).

---

## Next steps

- [QUICKSTART.md](QUICKSTART.md) — nothing to live savings in 5 minutes.
- [FEATURES.md](FEATURES.md) — every `ctx` command.
- [ARCHITECTURE.md](ARCHITECTURE.md) — pinned vs primed, how recall ranks.
- [ASD_INTEGRATION.md](ASD_INTEGRATION.md) — the suite, in depth.
- [LICENSING.md](../LICENSING.md) — OSS / Team / Enterprise editions.

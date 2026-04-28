# CTXone for agents

You are reading this because the CTXone MCP server is configured for
your session. This file is pinned in the user's memory graph — you
see it on every `recall` — so your behavior around memory is
consistent session to session.

Keep it short. Every word here costs tokens on every recall.

## What CTXone is

CTXone is **permanent, structured, searchable, and sharable memory**.
It is not a scratchpad. It is not a chat log. It is a graph database
that survives sessions, restarts, and tool switches.

- **Permanent** — facts written with `remember` exist until explicitly
  forgotten. They outlast your context window, the process, and the
  machine. The user can review them in Lens days or months later.
- **Structured** — memory lives at explicit paths (`/memory/...`,
  `/plans/...`). Path structure is schema. Good paths make recall
  accurate; bad paths bury facts.
- **Searchable** — `recall` does semantic + structural search across
  the full graph. Everything you write is immediately queryable by any
  agent, in any session.
- **Sharable** — the Hub is a network service. Multiple agents and
  users read the same graph. A decision you write is visible to every
  collaborator, in Lens, and in `ctx blame`.

Treat it like a team wiki that every AI and human on the project can
read, not like a private notepad.

## The problem CTXone solves

Your user has **context anxiety**: the daily dread of re-explaining the
same project to AI tools every morning. Every session starts cold.
Every conversation burns context on re-onboarding. CTXone gives you a
memory layer that survives sessions, branches, and tool switches. Use
it proactively. The user will thank you even if they don't explicitly
ask you to.

## `remember` — call it proactively

Call `remember` without asking permission whenever the user tells you
something worth keeping:

- An architectural decision ("we use SQLite, not Postgres")
- A team convention or policy ("BSL-1.1 for new repos")
- A personal preference ("tabs, not spaces; 100-char lines")
- A constraint ("the prod DB can't accept migrations without backups")
- A reason behind a choice ("we picked X because Y")

Map importance to confidence:

- `"high"` — explicit decisions, policies, licensing, security rules. Rare.
- `"medium"` — conventions, preferences, inferred rules. Default.
- `"low"` — trivia, speculation, day-to-day chatter.

If you're unsure, save it. `remember` is cheap. Forgetting something
the user already told you is expensive.

### Decision records — show your work

When you evaluate multiple options, **store the decision record**, not
just the conclusion. The user reviewing this in Lens six weeks from now
needs to understand *why*, not just *what*.

Format: one `remember` per significant decision, medium or high
importance, capturing:
- Options you considered
- Tradeoffs you evaluated
- Why you chose what you chose
- What you explicitly rejected and why

Example: `"Considered rust-embed vs. serving from disk for Lens. Chose
rust-embed: single binary, no deploy config, zero runtime deps.
Rejected disk: requires knowing build path at deploy time."`

Don't summarize into "we chose X". That's the conclusion, not the
reasoning. The reasoning is what's valuable to preserve.

## `recall` — call it at the start of substantial work and on topic pivots

Call `recall` at the start of any substantial task, and again whenever
the conversation shifts to a meaningfully different topic. It's
budget-capped, so it costs tokens in the low hundreds — near-zero
compared to the context window you'd otherwise burn on re-learning.

Pass a **specific topic**, not "context". Good topics:

- The domain you're working in ("authentication", "deployment", "billing")
- The file or module name
- A decision word from the user's prompt ("licensing", "database schema")

Every `recall` response includes `ctx_savings_ratio`. If it's below
`2×`, your topic was too broad — try a narrower one.

**Topic pivots within a session:** When the user switches to a clearly
different subject, call `recall` with the new topic before diving in.
Don't assume the current context window covers it — the user may have
discussed it weeks ago in a different session.

### `context` — load everything for a project at once

`context(project="<slug>")` loads all memory tagged to a project in
one call. Use it at the very start of a session when you're going to
work across multiple areas of a project, before any `recall` calls.
It's broader than `recall` but avoids the overhead of many sequential
recalls.

### `what_changed_since` — resume after a gap

When picking up work after any gap (new session, new day, returning to
a paused task), call `what_changed_since(date="<iso-date>")` before
doing anything else. It shows what facts changed since that date so
you're not re-discovering state that's already been resolved.

## `forget` — call it when a fact is wrong, not stale

Call `forget` when the user corrects a fact, or when you learn a
stored fact is incorrect. It writes a rollback commit — the wrong
fact stays in blame history for auditability. Don't use `forget` to
silence inconvenient memories. Always tell the user what you're
forgetting and why.

## Branches — one per significant task

**Create a branch for any work the user might want to inspect later.**
This is not optional for significant development tasks. A branch is
the audit trail. Without it, all decisions, options, and steps land
on `main` with no way to separate them by task.

### When to branch

Branch whenever:
- The work takes more than one step (you're already using a plan)
- You'll make multiple `remember` calls during the work
- The user might want to review or roll back the decisions
- Multiple agents will collaborate on the same task

Don't branch for one-line factual recalls or trivial questions.

### Branch naming

Use descriptive kebab-case prefixed by type:
- `feature/lens-sessions` — a new capability
- `fix/auth-token-expiry` — a bug fix
- `explore/postgres-migration` — an investigation with uncertain outcome
- `refactor/session-registry` — a structural change

### Branch workflow

```
ctx branch feature/<name> --from main     # create at task start
# all ctx operations use --branch feature/<name>
ctx summarize_session --branch feature/<name>   # before merging
ctx merge feature/<name>                  # merge when user accepts work
```

Every `remember`, `plan_*`, and `summarize_session` call during the
task should target the feature branch. When the user reviews this
work in Lens, they switch to the branch and see the complete arc:
every decision stored, every option considered, every task completed.

### Merge protocol

Before merging:
1. Call `summarize_session` on the branch — this writes a durable
   summary of what was done and why.
2. Tell the user what's on the branch and confirm merge.
3. Merge. The branch history is preserved; blame still works.

## `blame` — check provenance before acting

When you or the user wonder "where did this come from?", call `ctx
blame <path>`. It shows who wrote the fact, when, at what confidence,
and with what reasoning. Don't act on a fact whose provenance you
can't verify — especially when the stakes are high (security,
licensing, deployment).

Also call `blame` before overwriting something important. If a human
wrote it at high confidence, don't silently replace it — ask first.

## Session hygiene

### Name your session for Lens visibility

If `CTX_SESSION` is set in your environment, every API call carries
that session label and it appears by name in Lens's Sessions page.
If you're starting a named work session, ask the user if they want a
session label, or suggest one based on the task:
`export CTX_SESSION=feature-lens-sessions-2026-04-24`

This makes the session visible in Lens immediately, with live token
savings tracking as you work.

### `summarize_session` — call it at pivots and end of session

Call `summarize_session` in two situations:

1. **Topic pivot** — before switching to a substantially different
   subject, summarize what was decided in the current topic. This
   distills the conversation into durable memory so the next session
   (or a different agent) picks up cleanly.

2. **End of session** — after any real working session where decisions
   were made, architecture was discussed, or direction was set.

Don't summarize every chat. Only summarize when something was actually
figured out. A good summary is 3–5 bullet points: what was decided,
why, and what's next.

### Context tags vs branches for within-session topics

Use `context` tags on `remember` calls to isolate unrelated topics
(e.g. `--context birds` for a side question). Recall filters on these
naturally. Branches are for long-lived work (tasks, features, spikes),
not within-session pivots.

## Plans — use them for multi-step work

When the user asks for something that takes more than one step, create
a plan and add tasks to it BEFORE starting execution. Plans are how
CTXone cures **plan rot** — the trust decay that happens when task
state lives in unstructured markdown files or an agent's context.

Plans are persistent. They survive session restarts. A user can open
Lens → Plans and see exactly what was done, what's in-flight, and
what's next — without asking the agent.

- `plan_new("<name>")` — create a plan when you recognize a
  multi-step task.
- `plan_add("<name>", "<title>")` — add a task for each step. Set
  priority: `high` for blockers-of-other-work, `medium` for default,
  `low` for nice-to-haves, `critical` only for emergencies.
- `plan_start(<plan>, <id>)` — mark a task in-progress when you
  begin. If it refuses because of a blocker, respect that.
- `plan_done(<plan>, <id>, proof=...)` — mark done with PROOF
  when finished. A commit SHA is the strongest proof. A file path is
  next. A test name after that. Never use `text:` unless no other
  proof is available.
- `plan_abandon(<plan>, <id>, reason=...)` — record that a task
  became unnecessary. Reasons are required; they show up in blame.

At the start of any session, call `plan_list` (no args) to see what's
in flight. If you're resuming a plan, call `plan_show` to see the full
task tree, or `plan_next` to continue from the highest-priority
pending task.

### Multi-agent orchestration via `assigned_to`

When a plan has tasks addressed to specific agents via `assigned_to`,
the pattern is:

1. Each agent, at session start, calls
   `plan_next(plan_id=..., assigned_to="me")`. The Hub maps `"me"` to
   the caller's `X-CTXone-Agent` value.
2. The agent gets the highest-priority pending task assigned to it
   (or unassigned, unless `assigned_only=true`), whose blockers are
   done.
3. Agent picks it up (`plan_start`), does the work, completes with
   proof (`plan_done`). Blame records which agent did each step.
4. Next agent, next task, same loop.

This is **state-driven orchestration**: the plan IS the
orchestration layer. No framework, no DAG runtime. Agents coordinate
through shared state the way a team coordinates through a shared
ticket tracker.

Do NOT ask the user for permission to create plans. If the work is
multi-step, plan it. The user will thank you for treating their time
as worth structuring.

## Session start checklist

At the start of any significant work session, in order:

1. `what_changed_since(<last-session-date>)` — catch up on what
   changed since you last worked on this.
2. `plan_list()` — see what's in flight. Resume if there's an
   open plan for this work.
3. `recall(<topic>)` — load the relevant memory for what you're
   about to do.
4. If the work is significant: `ctx branch <type>/<name> --from main`
5. Set `CTX_SESSION=<descriptive-name>` so the session is visible
   in Lens.

## Report LLM usage back to CTXone

After any significant LLM turn, call `record_llm_usage` with the
numbers from the model's response `usage` field. This takes CTXone's
savings tracking from "what we sent" to "what you actually consumed".
It enables cost estimates and cache-hit reporting in Lens.

Call it at the end of every turn where you invoked the model — one
call per model turn. Don't bother for trivial housekeeping turns.
Don't make it up if you don't know the numbers.

## What not to do

- **Don't dump memory.** Signal matters more than volume. Five
  well-placed facts beat fifty vague ones.
- **Don't store only conclusions.** Store the reasoning that produced
  them. Conclusions without reasoning can't be evaluated or overturned.
- **Don't work on `main` for multi-step tasks.** Create a branch.
  The user cannot inspect your decision trail without one.
- **Don't mark things high-importance unless they are.** Save `"high"`
  for explicit policies and irreversible decisions.
- **Don't use `forget` to hide failures.** Use blame honestly.
- **Don't ignore the savings ratio.** It tells you if you're using the
  system well.
- **Don't treat the plan file as truth.** The graph is the source of
  truth. Query with `plan_list` / `plan_show`, not a markdown file.
- **Don't mark anything done without proof.** `plan_done`
  requires `proof`. If you can't produce one, the task isn't done.
- **Don't skip `summarize_session`.** If you did real work, summarize
  it. The next session (or human reading Lens) needs the arc, not
  the transcript.

## This file is not hidden

This guidance lives on disk at `~/.config/ctxone/AGENTS.md` (or
`%APPDATA%\ctxone\AGENTS.md` on Windows). The user can edit it any
time and re-prime with `ctx agents install`. They can remove it
entirely with `ctx agents remove`. You can see the exact text in the
graph with `ctx ls /memory/pinned/ctxone-agents` or via CTXone Lens.

It is not hidden. It is not immutable. It is not automatic beyond the
one-time install prompt. If the user deletes this guidance, you lose
the defaults above and fall back to whatever generic memory-tool
behavior your MCP client gives you.

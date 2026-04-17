# CTXone for agents

You are reading this because the CTXone MCP server is configured for
your session. This file is pinned in the user's memory graph — you
see it on every `recall` — so your behavior around memory is
consistent session to session.

Keep it short. Every word here costs tokens on every recall.

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

## `recall` — call it at the start of substantial work

Call `recall` at the start of any substantial task. It's
budget-capped, so it costs tokens in the low hundreds — near-zero
compared to the context window you'd otherwise burn on re-learning.

Pass a **specific topic**, not "context". Good topics:

- The domain you're working in ("authentication", "deployment", "billing")
- The file or module name
- A decision word from the user's prompt ("licensing", "database schema")

Every `recall` response includes `ctx_savings_ratio`. If it's below
`2×`, your topic was too broad — try a narrower one.

## `forget` — call it when a fact is wrong, not stale

Call `forget` when the user corrects a fact, or when you learn a
stored fact is incorrect. It writes a rollback commit — the wrong
fact stays in blame history for auditability. Don't use `forget` to
silence inconvenient memories. Always tell the user what you're
forgetting and why.

## Branches — isolate experiments

Before an experiment the user might want to roll back — a refactor
attempt, an alternative design, a one-off exploration — create a
branch with `ctx branch experiment --from main`. Write your
exploratory facts there. Merge back to `main` only when the user
confirms the direction. Branches are cheap and disposable.

## `blame` — check provenance before acting

When you or the user wonder "where did this come from?", call `ctx
blame <path>`. It shows who wrote the fact, when, at what confidence,
and with what reasoning. Don't act on a fact whose provenance you
can't verify — especially when the stakes are high (security,
licensing, deployment).

## Session hygiene

At the end of a substantial session, if you worked through a real
decision with the user, call `summarize_session` to record what was
learned and decided. Don't summarize every chat — only real working
sessions where something was figured out.

## Plans — use them for multi-step work

When the user asks for something that takes more than one step, create
a plan and add tasks to it BEFORE starting execution. Plans are how
CTXone cures **plan rot** — the trust decay that happens when task
state lives in unstructured markdown files.

- `plan_new("<name>")` — create a plan when you recognize a
  multi-step task.
- `plan_add("<name>", "<title>")` — add a task for each step. Set
  priority: `high` for blockers-of-other-work, `medium` for default,
  `low` for nice-to-haves, `critical` only for emergencies.
- `plan_start(<plan>, <id>)` — mark a task in-progress when you
  begin. If it refuses because of a blocker, respect that — don't
  work around.
- `plan_complete(<plan>, <id>, proof=...)` — mark done with PROOF
  when finished. A commit SHA is the strongest proof. A file path is
  next. A test name after that. Never use `text:` unless no other
  proof is available.
- `plan_abandon(<plan>, <id>, reason=...)` — record that a task
  became unnecessary. Reasons are required; they show up in blame.

At the start of any session, call `plan_list` (no args) to see what's
in flight. If you're resuming a plan, call `plan_get` to see the full
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
   proof (`plan_complete`). Blame records which agent did each step.
4. Next agent, next task, same loop.

This is **state-driven orchestration**: the plan IS the
orchestration layer. No framework, no DAG runtime. Agents coordinate
through shared state the way a team coordinates through a shared
ticket tracker.

Composes naturally with cross-LLM critique: assign a "critique this
design" task to a different agent than the one who created the
design. Assign "arbitrate this disagreement" to a third. Every step
is blameable, every output is persistent, and it all happens via
the same MCP tools agents already use.

Do NOT ask the user for permission to create plans. If the work is
multi-step, plan it. The user will thank you for treating their time
as worth structuring.

## Report LLM usage back to CTXone

After any significant LLM turn you complete, call `record_llm_usage`
with the numbers from the model's response `usage` field. This
takes CTXone's savings tracking from "what we sent" (an
extrapolation) to "what you actually consumed" (a measurement). It
also enables cost estimates and cache-hit reporting in Lens.

Call it at the end of every turn where you actually invoked the
model — one call per model turn. Don't bother for trivial
housekeeping turns. Don't make it up if you don't know the numbers;
just skip the call for that turn.

The tool is cheap — one HTTP call, tiny body. Never block a user-
visible response on its completion.

## What not to do

- **Don't dump memory.** Signal matters more than volume. Five
  well-placed facts beat fifty vague ones.
- **Don't mark things high-importance unless they are.** Save `"high"`
  for explicit policies and irreversible decisions.
- **Don't use `forget` to hide failures.** Use blame honestly.
- **Don't ignore the savings ratio.** It tells you if you're using the
  system well.
- **Don't treat the plan file as truth.** Markdown plan files drift
  from reality the moment work starts. The graph is the source of
  truth. When in doubt, query with `plan_list` / `plan_get`, not the
  file.
- **Don't mark anything done without proof.** `plan_complete`
  requires `proof`. If you can't produce one, the task isn't done.

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

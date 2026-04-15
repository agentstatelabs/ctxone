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

## What not to do

- **Don't dump memory.** Signal matters more than volume. Five
  well-placed facts beat fifty vague ones.
- **Don't mark things high-importance unless they are.** Save `"high"`
  for explicit policies and irreversible decisions.
- **Don't use `forget` to hide failures.** Use blame honestly.
- **Don't ignore the savings ratio.** It tells you if you're using the
  system well.

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

# Memory Branch Scoping

> Decision record for plan `lens-enhancements` / **t-007**.
>
> **Status:** Accepted (2026-04-27).
> **Audience:** anyone reasoning about where `remember` writes go and
> what `recall` returns when an agent is working on a non-`main` branch.

## TL;DR

- **Memories live on whatever branch the caller writes to.** They are
  ordinary entries under `/memory/**` in the AgentStateGraph, indexed
  the same way as any other state.
- **Default branch is `main`.** Every memory tool — HTTP, MCP, CLI —
  defaults `ref` to `"main"` unless the caller passes one explicitly.
- **`recall` is single-branch.** It does **not** transparently union
  facts from `main` + the working branch. The caller picks the ref.
- **Branch merges merge memory.** Because memories are AgentStateGraph
  paths, they participate in the same JSON-Patch diff/merge that
  `/api/diff` and `/api/merge` already use for everything else. No
  special-cased "memory merge" path exists.

This is the simplest model that is consistent with the rest of the
graph; the "everything is just a path" invariant is more valuable than
any specific ergonomic improvement we could bolt on per-domain.

## Why this question exists

Before t-007 it was easy for an agent to do:

```
ctx --branch feature/x remember "foo"
ctx recall foo            # ← reads main; foo is invisible
```

…and conclude that `remember` was broken. It isn't — the memory is on
`feature/x`. But the surface didn't make the branch axis obvious, and
a number of judgement calls were unstated:

- Should `remember` *always* write to `main` (so memories are global)
  or to whatever branch the caller is on?
- When recalling on a feature branch, should we surface facts written
  on `main` even if they don't exist on the branch yet?
- When two branches both `remember` overlapping facts and we merge,
  what happens — overwrite, conflict, or de-dupe?

## Options considered

### A. Memories always live on `main`

Every `remember` writes to `main` regardless of the caller's branch.
Recall is implicitly cross-branch because there's only one branch's
worth of memory.

- ✅ Simple recall — no branch parameter to think about.
- ❌ Breaks the "branch is a sandbox" invariant. Speculative work
  can pollute the canonical store before it's accepted.
- ❌ No way to capture "facts that are only true if this branch
  lands" (e.g. "API endpoint renamed from X to Y on this branch").
- ❌ Loses the ability to demo/explore on a branch and discard the
  branch without scrubbing memory.

### B. Memories follow the working branch (current behavior)

Each `remember` writes to the caller-specified `ref` (default `main`).
Recall reads from one ref at a time. Merges propagate memory
alongside everything else.

- ✅ Branches are uniformly sandboxed — code, plans, **and** memory.
- ✅ Merges already do the right thing (memory paths diff like any
  other path).
- ✅ Speculative facts stay speculative until merged.
- ⚠️ Caller has to think about `--branch` when reading and writing.
- ⚠️ Two agents working in parallel branches won't see each other's
  memories until merge.

### C. Hybrid — write to current branch, recall walks branch → main

Write goes to the working branch. Recall searches the working branch
first, then falls back to `main` for any path not present on the
branch.

- ✅ Best of both worlds for *reading*: a feature branch sees its
  own facts plus everything on `main`.
- ❌ Recall behavior becomes harder to explain ("why did I get this
  fact? it's on main but my branch has a different value for the
  same path"). Conflict resolution at recall time has to be
  invented.
- ❌ Two-pass recall doubles read cost.
- ❌ Subtly different semantics from `state get` / `log` / `blame`,
  which are single-ref. Memory would become the only domain that
  "leaks across branches."

## Decision

**Adopt Option B.** Memories live on whatever branch the caller is
currently working on; merges carry them across branches via the
existing JSON-Patch diff/merge machinery; `recall` is single-ref.

The hybrid (C) is *not ruled out forever* — it's an opt-in flag we
can add later (`recall --include main` or similar) without changing
the storage model. We're explicitly choosing **not** to ship it as
the default because the explainability cost is high and the
single-ref model matches every other CTXone tool.

## What this means in practice

### For agents (writing)

- `remember`/`forget`/`prime` write to `ref` (default `main`).
- If you want a fact to be visible everywhere, write it on `main`:
  `ctx remember "foo"` (no `--branch`).
- If you're on a feature branch and the fact is only true *if this
  branch lands*, pass `--branch feature/x` (or use the per-session
  default branch via `branchStore` in Lens / `CTX_BRANCH` in CLI).

### For agents (reading)

- `recall` reads from one ref. Default is `main`.
- To see facts written on a feature branch, pass `--branch feature/x`.
- `recall` does **not** transparently union with `main`. If you
  need both, run two recalls or merge the branch.

### For Lens

- The branch picker in the top bar (`branchStore`) already governs
  which ref every page sees. Memory pages (Pinned, Sessions,
  Memories tree) follow that picker — same as State, Log, etc.
- The Sessions page groups by `session:<id>` tag, which is
  branch-scoped (the tag is part of the memory entry, not a
  separate index).

### For merges

- Memory paths diff exactly like any other path. `GET /api/diff`
  shows `add`/`remove`/`replace` ops on `/memory/**`.
- `POST /api/merge` applies them in the same commit boundary as
  code changes. No "memory merge" code path.
- Conflict (same path edited on both sides with different values)
  surfaces through the existing 409 → `MergeConflict` envelope. The
  diff page already renders this.

### For `forget`

- `forget` deletes on the specified ref. Forgetting on `feature/x`
  does **not** propagate to `main`; merge the branch (or run
  `forget` on `main` directly) if you want it gone everywhere.
- This is consistent with branch-scoped writes — forgetting is just
  a `delete` op.

## Code touchpoints (no changes required for this decision)

The current implementation already matches the decision:

| Surface | File | Behavior |
| --- | --- | --- |
| HTTP `remember` | `server/src/http.rs` `remember()` | `default_ref()` → `"main"`; `req.ref_name` honored |
| HTTP `recall` | `server/src/http.rs` `recall()` | `q.ref_name` honored; `run_recall(...,&q.ref_name)` |
| HTTP `forget` | `server/src/http.rs` `forget()` | `req.ref_name` honored |
| MCP `remember`/`recall`/`forget` | `server/src/memory_tools.rs` | Each param struct has `#[serde(default = "default_ref", rename = "ref")]` |
| CLI | `cli/src/main.rs` | `--branch` flag flows through to every memory subcommand |
| Lens | `web/src/lib/branchStore.svelte.ts` | Top-bar picker drives `ref` for all memory pages |

So this t-007 change is **doc-only**. The decision exists to make the
implicit explicit: anyone reading this doc should be able to predict
what `remember` and `recall` will do without spelunking source.

## Open questions / future work

1. **Cross-branch recall.** If we add it, ship it as
   `recall --include-refs main,feature/x` rather than implicit
   fallback. Result envelopes should attribute each fact to its ref
   so the consumer can disambiguate.
2. **Memory promotion.** No "promote this memory from feature/x to
   main without merging the whole branch" tool exists. If demand
   surfaces, model it as `forget(branch) + remember(main)` plus a
   blame note.
3. **Per-branch session aggregates.** The Sessions page already
   filters by ref. We may want a "facts written on this branch
   since it diverged from main" view — that is `diff(main, branch)`
   filtered to `/memory/**`, which the diff endpoint can answer
   today.
4. **Default branch policy.** Today every tool defaults to `"main"`.
   We may want a per-agent default ref (env var or config) so an
   agent that "lives on" `feature/x` doesn't have to thread
   `--branch` through every call. Out of scope for this decision.

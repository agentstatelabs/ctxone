# Design: Hub Sync — your memory on every machine, shared with your team

**Status:** built for your own account (updated 2026-11-07), on
AgentStateGraph 1.5.3. Each machine's Hub pushes to and pulls from a Hub that
serves sync ([CONNECT_A_HUB.md](../CONNECT_A_HUB.md#sync-your-own-hub-with-a-remote-one)).
Still to come: commits carrying the device id (t-016), sharing a workspace with
a team (waits on AgentStateGraph Plan F t-005), device-code login, and opt-in
turn sync. See [Tasks](#tasks). · **Plan:** `ctx-hub-sync`
**Builds on:**
- AgentStateGraph Plan G: git-style push/pull, a control ref for taints, reminders and epochs, and push guards.
- AgentStateGraph Plan F: the embedding API (per-call context, many stores per process).
- AgentStateDeveloper Plan W: judgment on its own ref.

## Goal

Two outcomes:
- A CTXone user's memory follows them to every machine they work on.
- A project's memory can be shared with a team.

Both use AgentStateGraph's git-style push and pull against a hub. Local-first
stays the default: a hub is something you connect, never something CTXone needs.
The first outcome is built; the second waits on AgentStateGraph scoping the
control ref's records by workspace (see [Remotes per namespace](#remotes-per-namespace)).

## Decisions

1. **Sync anything hard or impossible to reproduce.** Anything regenerable stays
   local.
2. **Raw session turns stay local by default.**
   - They move to their own local-only ref per namespace. The session envelope
     (title, meta, provenance, burn, analysis, summary, decisions, links,
     tombstones, and a new usage rollup) syncs.
   - A per-project opt-in for turns can come later, behind the credential scan
     (decision 6).
3. **A personal namespace, separate from project namespaces.**
   - Your agent rules (`AGENTS.md`), personal preferences and your project
     registry live in a per-user namespace that syncs only to your own hub space.
   - Project namespaces can sync to your own space or to a team's.
   - Recall reads both.
4. **A remote per namespace,** the way git sets a remote per branch. Every branch
   inside a synced namespace syncs. Branches mirror git branch names, so
   teammates on the same git branch share that memory branch.
5. **Colliding task ids are renumbered before push, git-rebase style.**
   - Unpushed local tasks whose `t-NNN` was taken remotely move past the remote's
     ids, with references rewritten, before the push.
   - Nothing already shared is ever renumbered.
   - This is an AgentStateGraph task-store change.
6. **Pushes are scanned for credentials.** High-confidence matches block the
   push, with an explicit per-finding override; the report names the path.
   AgentStateGraph refuses `/_meta/_secret` outright. A real store held
   credential-shaped strings in both transcripts and `/memory`, so the path rule
   alone isn't enough.
7. **The first push sends current state only** (a shallow boundary). Existing
   history, including turn history, never leaves the machine.

Decided 2026-10-29, after the multi-tenant Hub shipped:

8. **A multi-tenant Hub is the sync remote.** This design first assumed a hub
   per user. One multi-tenant `ctxone-hub` now serves many users from one
   store, so it serves AgentStateGraph's sync for each tenant, behind that
   tenant's token (t-015), and `ctx remote` points at it.
9. **AgentStateGraph ships the sync server as an embeddable service** with an
   auth hook, which `ctxone-hub` mounts. Its single-account `--hub` binary is
   built on the same service.
10. **The control ref is `_control`.** Its merge rules: a resolved taint stays
    resolved; reminder status changes are ordered, and otherwise the newest
    edit wins; a sealed epoch never changes.
11. **ASD judgment syncs to the same Hub,** per tenant, beside the tenant's
    CTXone memory. ASD's code index stays on each machine
    (AgentStateDeveloper Plan W).

## What we measured

One real store, 2026-10-06, read-only.

| | Size |
|---|---|
| Database file | 2.28 GB |
| Unique current state, all 352 refs in 31 namespaces | 317 MB |
| `/sessions` | 294 MB, almost all of it `turns` |
| `/plans` | 17.7 MB |
| `/memory` | 3.9 MB |
| `/ctxone` | 1.4 MB |
| `/session_links` | 0.3 MB |
| History-only objects | 874 MB (73% of object bytes); turns introduced ~364 MB of it |
| Search index (`leaf_*` tables, regenerable) | ~770 MB |

- **What syncs is small.** Moving turns off the synced refs takes synced
  current state from ~317 MB to ~24–30 MB.
- **Most sessions have no LLM analysis.** 86 of 567 sessions have one; the
  envelope (title, meta) covers nearly all of them.
- **A third of commits change nothing.** 33% of commits leave the tree as it
  was (re-imports, identical full-turn rewrites, title/provenance/burn
  rewrites). Skipping those writes cuts history growth for free.

## What syncs

| Path | Sync? | Notes |
|---|---|---|
| `/memory/**`: facts, `<context>`, pinned, primed, curated | Yes | Authored and LLM-extracted facts share one path shape; extraction is costly and non-deterministic to redo. Credential-scanned. |
| `/drafts/<sid>/*` | Yes | Small review queue. |
| `/sessions/<sid>/{title, meta, provenance, burn, usage, analysis, summary, decisions, details}` | Yes | The session envelope, ~1.7 KB each. `usage` is the usage rollup (t-005). |
| A session's raw turns: `/turns/<sid>/**` on `_local/turns` (they were at `/sessions/<sid>/turns`) | **No** (local-only ref) | Re-scrapable only on the originating machine, while its transcript files exist. Opt-in later. |
| `/session_links/*`, `/session_tombstones/*` | Yes | Tombstones make deletions propagate and stop re-imports. |
| `/plans/**`, `/plan_links/**` | Yes | Task ids renumbered before push (decision 5). |
| `/docs/*`, `/ctxone/branches/*` | Yes | Small. |
| `/ctxone/schema_version` | Yes | The hub uses it as a compatibility gate. |
| `/_meta/*` | No | Owned by AgentStateGraph. |
| Taints, reminders, epochs | Yes, via the ASG control ref | Not yet scoped by namespace (ASG Plan F t-005), so every workspace syncs to your own space. A reminder runs its commands only where it was made or approved; seals from before workspaces stay local. See "Side tables". |
| `projects` table: id, remote, display name | Yes, in the personal namespace | Today it's SQL outside the graph, so it wouldn't sync. |
| `project_paths` table (absolute paths) | No | Machine-local. |
| `ctxone_sessions` usage table | Rolled up into the envelope | The hub needs usage for plan cost and savings views. |
| `~/.ctxone/*`: config, keys, `extraction.toml`, `ignored-sessions.txt`, `reminder-tick.allow`, backups | No | Machine-local. Some hold secrets or local trust decisions. |

## Model

### Identity

- **Project id:** taken from `.ctxproject`. Failing that, from the **full**
  normalized git remote (`host/owner/repo`, with ssh and https collapsed).
  - Auto-derivation used only the last path segment, so
    `github.com/a/app` and `github.com/b/app` both became `app`.
  - Existing ids are kept. The fix applies to new auto-derived ids, and the
    personal registry records the remote, so ids stay mappable.
  - Done (t-001): new ids are the kebab-case canonical remote
    (`github-com-owner-repo`; one over 64 bytes drops its host and ends in
    a hash). The registry keeps remotes in that canonical form, so an ssh
    clone finds a project registered from https; the Hub rewrites older
    remotes when it starts or opens a tenant, safe to run again. Host
    aliases (an IP and a DNS name, `~/.ssh/config` aliases) aren't
    resolved.
- **Project registry:** the id → remote and display-name mapping moves into the
  personal namespace (`/ctxone/projects/<id>`), so it follows the user. Local
  paths stay in the SQL table.
  - Done (t-003): each project's record holds its namespace, remote and
    display name, not its paths or when this Hub registered it (that differs
    per machine). `POST /api/projects` writes it, and every start and tenant
    open brings the two together: a project here gets its record, this Hub's
    row winning, and a record with no row here (one a sync brought) is
    registered without paths. Unchanged records write nothing.
  - The SQL table stays the index detection reads, since it must be readable
    before any namespace is opened.
- **Device:** each install gets a device id, used to attribute commits and pushes.
  Done for the id itself (t-001): `~/.ctxone/device_id`, minted by `ctx init`
  or first use, and carried in each session's provenance. Commits get it when
  AgentStateGraph can take it per call: plan commits are made inside
  AgentStateGraph with only an agent id (t-016).
- **Account:** comes from hub login (device-code flow); the token is kept in the
  OS keychain.

### Personal namespace

- A reserved per-user namespace holds:
  - `/memory/pinned/ctxone-agents/*`, primed from `AGENTS.md` here instead of
    into every project;
  - personal preferences, via `remember` with a personal scope;
  - the project registry.
- Recall and `prime` read the personal namespace plus the project namespace.
- The personal namespace syncs only to the user's own space, never a team's.
- Done (t-003): the namespace is `_personal`, beside
  every workspace in each store (each tenant's, on a multi-tenant Hub), on
  `main` only. Names beginning with `_` are reserved, so no project takes it.
  - `ctx agents install` primes the rules there and removes the workspace's
    own copy; `ctx agents migrate --apply` removes the copies earlier
    versions primed into every workspace.
  - `ctx remember --personal`, and `personal: true` on MCP `remember`,
    `forget` and `prime`. Over REST it's `?namespace=_personal`.
  - Recall, the per-task recall gate and `GET /api/memory/pinned` read the
    workspace and `_personal`; each result says which, and the workspace's
    copy of a path wins.
  - Lens lists it as Personal and marks what comes from it.
  - The project registry's records are at `/ctxone/projects/<id>` (see
    Identity).

### Remotes per namespace

- **Commands:**
  - `ctx remote add <name> <url>` registers a hub;
  - `ctx remote set <namespace> <remote>` maps a namespace to it;
  - `ctx push` / `ctx pull` / `ctx sync status`.
- **Where namespaces go:** the personal namespace goes to the user's own space;
  a project namespace goes to the user's own space or to a team's. On a
  multi-tenant Hub, a space is a tenant (decision 8).
- **Background sync:** the hub daemon is the only writer to the db, so it pushes
  and pulls in the background on an interval and after writes. Lens shows
  ahead/behind status per namespace.
- Done (t-006, t-015):
  - **The serving side:** `ctxone-hub --serve-sync` mounts AgentStateGraph's
    `/sync/v1/<store>` router behind the Hub's own auth. A token reaches its
    own tenant's store only, and `<store>` is `me` or the tenant's name
    (`default` on a single-tenant Hub).
  - **The client:** the Hub daemon is the client too, and `ctx` asks it
    (`/api/sync/*`).
    - Remotes are AgentStateGraph's named remotes. A token lives in
      `~/.ctxone/sync-tokens/<remote>` (0600) or an environment variable,
      never in the store.
    - The mapping and each workspace's boundaries sit on `_local/sync`. The
      boundaries are the branch and control heads when the workspace was
      mapped; no push goes past them.
  - **A sync** fetches, renumbers colliding tasks, and joins each branch
    (AgentStateGraph's `join_fetched`, so two stores that each began on their
    own merge against an empty base). Then it pushes each branch through the
    credential scan, shallow from the boundaries.
  - **The background loop** runs every `CTXONE_SYNC_EVERY` seconds (default
    300) and within a minute of a change.
  - **Own account only:** a mapping to another space is refused, and mapping
    the first workspace maps the personal namespace too. The control ref goes
    with every push, and its taints and reminders have no workspace.

### Turns on their own ref

- **What moves:** every turn writer moves turns to a local-only ref per
  namespace. That covers the HTTP bulk and per-turn routes, `ctx ingest-session`,
  the Stop-hook `capture-turn` path, the hub sweep and Lens import.
  - Done (t-002): turns are at `/turns/<sid>/t<NNNN>` on `_local/turns`,
    whatever branch the session ran on. Every reader, a session's delete and
    move, and the secrets scan's `--turns` use it.
  - Schema 3 moves existing turns off every branch, relinked rather than
    copied: a 900 MB store with 300 MB of turns took 8 seconds.
- **What stays on `main`:** the session envelope.
- **Readers:** they already avoid turns. Recall reads `/memory`, discovery is
  depth-limited and `why_did_we` drops `/sessions`, so nothing that syncs
  depends on turns.
- **Usage:** a per-session usage rollup (tokens by model, the last model, and
  the `tokens_sent` estimate) is written into the envelope at
  `/sessions/<sid>/usage`. `ctx` works out the split from the transcript at
  ingest, so a Hub that gets no turns has it too. Plan cost and the session
  list then work on any machine without turns or the local `ctxone_sessions`
  table. Dollar costs aren't stored: Lens prices tokens when it reads them,
  and a stored figure would go stale with the price table.

### Plans and tasks

- **Task ids:** on pull, unpushed local tasks whose ids were taken remotely are
  renumbered past the remote's ids before push. `blocked_by`, parent ids and
  `plan_links` are rewritten with them.
- **Plan names:** if two people create a plan with the same name, the result
  surfaces as a merge conflict through the existing structured conflict payload.

### Side tables

Taints, reminders and epochs ride AgentStateGraph's control ref, `_control`
(Plan G t-001), with namespace scoping (Plan F t-005) and the merge rules in
decision 10. Today taints and reminders are SQL tables with no namespace,
shared by every workspace in a store, as
[MEMORY_BRANCH_SCOPING.md](../MEMORY_BRANCH_SCOPING.md) says.

- **Reminders:** a synced reminder never runs a command on a machine unless it
  was created there or approved there. `reminder-tick.allow` stays local.
  Done (t-008): the reminders made or approved through a Hub are on its
  `_local/trust` ref; every reminder it lists says `trusted_here`, and the tick
  runs only those. Schema 4 trusts what a store held before.
- **Epochs:**
  - **Legacy seals stay local:** 171 have no namespace, and their sealed-commit
    lists total ~82 MB, referencing turn history.
  - **New plan epochs sync.** They seal only synced refs, so a hub can verify
    them.
  - Done (t-009): schema 5 moves the epochs with no workspace to
    AgentStateGraph's local control ref (`keep_epochs_local`). Plan epochs
    sync; a seal over history from before sync binds only on a store that
    holds those commits (AgentStateGraph 1.5.3).

### Attribution

- Over HTTP MCP, commits were stamped with the daemon-wide agent id. `ctx init`
  now writes `X-CTXone-Agent` per tool (`?agent=` for Codex, whose config has
  no headers), as [UNIFIED_HUB.md](UNIFIED_HUB.md) planned, and the Hub stamps
  each MCP session's commits with it (t-010).
- Commits will also carry the device id, via the AgentStateGraph Plan F call
  context (t-016).

## Tasks

| Task | Description | Wave | Status |
|------|-------------|------|--------|
| t-001 | **Identity.** New auto-derived project ids use the full normalized remote. Add a device id at install. Move the project registry's id, remote and display name into the personal namespace; local paths stay in SQL. | 1 | Done |
| t-002 | **Turns on their own local-only ref.** Redirect every turn writer. The envelope stays on `main`. Migration: the synced refs start from a fresh commit, and existing turn history stays local. | 1 | Done |
| t-003 | **Personal namespace.** Reserve it. Prime `AGENTS.md` rules into it. Add a personal scope to `remember`. Recall and `prime` read personal plus project. | 1 | Done |
| t-004 | **Stop no-op writes.** Skip commits that leave the tree unchanged (re-imports, identical full-turn rewrites, unchanged title/provenance/burn). | 1 | Done |
| t-005 | **Usage rollup in the session envelope,** so plan cost and savings views work from synced data. | 1 | Done |
| t-006 | **`ctx remote add / set`, `ctx push / pull`, `ctx sync status`,** plus daemon background sync and Lens status, on AgentStateGraph Plan G. Hub login via device code; token in the OS keychain. | 2 | Done; device-code login waits on Hub accounts |
| t-007 | **Credential scan before push.** Use AgentStateGraph's push filter with CTXone's patterns: block high-confidence matches, with a per-finding override. Add `ctx secrets scan` to find and clean matches in a store before its first push. | 2 | Done |
| t-008 | **Reminder safety.** Synced reminders never auto-run commands unless created or approved on this device. | 2 | Done |
| t-009 | **Epochs.** Legacy seals stay local; new plan epochs ride the control ref and seal only synced refs. | 2 | Done |
| t-010 | **Attribution.** `ctx init` writes `X-CTXone-Agent`; the device id goes on commits. | 2 | Done; the device id on commits is t-016 |
| t-011 | **Hub-mode gaps on Postgres.** Today the project registry returns 400 and session stats aren't persisted on Postgres. A hub needs both, plus read-only access to synced ASD stores (AgentStateDeveloper Plan W t-008). | 3 | Done |
| t-012 | **Docs.** Add a "connect a hub" guide. Correct MEMORY_BRANCH_SCOPING (reminders and taints aren't namespace-isolated until the control ref lands), COST_SAVINGS ("every machine resolves to the same graph" holds only with one shared hub), DEPLOYMENT, UNIFIED_HUB's default transport, and the `workspace.rs` doc comment. | 3 | Done |
| t-013 | **A remote hub never receives raw turns unless the user opts in.** When the configured hub isn't loopback, `ctx ingest-session`, the Stop-hook `capture-turn` path and imports send the session envelope and extracted memories, not `turns`. This applies to hosted hubs as well as sync, so decision 2 holds when a client talks to a hub directly. | 1 | Done |
| t-014 | **Hosted-mode hardening.** Add an option to turn off the loopback exemption: today loopback peers skip the bearer token, so a proxy or sidecar on 127.0.0.1 would bypass auth. Add token rotation without a restart, and an explicit origin allow-list for a hosted Lens. | 1 | Done |
| t-015 | **`ctxone-hub` serves sync for each tenant,** mounting AgentStateGraph's sync service behind the tenant's token (decisions 8 and 9). | 2 | Done |
| t-016 | **Commits carry the device id.** `ctx` sends it, and the Hub sets it on every commit through AgentStateGraph's per-call context. | 2 | Waits on AgentStateGraph |
| t-017 | **`ctx secrets scan`** finds credential-shaped strings in what would sync, with a masked match and the `ctx forget` for each. Split from t-007. | 1 | Done |
| t-018 | **Docs before sync:** [CONNECT_A_HUB.md](../CONNECT_A_HUB.md), and the corrections t-012 listed that hold today. | 1 | Done |

### Depends on AgentStateGraph

| CTXone task | Needs |
|---|---|
| t-002 local-only ref | Plan G t-005 (local-only push guard) |
| t-006 | Plan G t-004 and t-006 (library push/pull, HTTP transport and hub) |
| t-007 | Plan G t-011 (pluggable push filter) |
| Decision 5 | Plan G t-010 (merge-safe task ids) |
| t-008, t-009 | Plan G t-001 (control ref) and Plan F t-005 (namespace scoping) |
| t-015 | Plan G t-004 and t-006, with the sync server as an embeddable service |
| t-016 | Plan F's per-call context (or a task-store hook) carrying a device id |
| All of the above | An AgentStateGraph release containing them: 1.5.1 brought Plan G, and 1.5.3 joining unrelated histories, seals over history a store never received, and local epochs. CTXone is on 1.5.3. |

## Not in this design

- Hub accounts, billing and hosting operations.
- Opt-in turn sync (it comes later, behind t-007).
- Team roles and access control.
- Hosted code tools: ASD's index stays local (Plan W).
- Retention and GC of the local turns ref.

## Done when

- A user connects a hub, and their memory, plans, session summaries, reminders
  and agent rules appear on their other machines.
- A team shares a project namespace without overwriting each other's rules or
  tasks.
- No raw transcript or credential-shaped string leaves a machine without an
  explicit choice.
- A user who never connects a hub sees no change.

# Changelog

All notable changes to CTXone are documented here. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning follows
the project's `0.9.x` series (incremented by 0.0.01 per release).

## [v0.9.33] — 2026-08-12

### Added
- **Opt-in session import onboarding.** The background sweep that pulled every transcript on the machine into the hub is now **off by default** — a fresh hub no longer silently ingests a machine's entire agent history. When enabled (`CTXONE_SESSION_SYNC_INTERVAL_SECS`), the sweep is **chunked per source** (claude/codex/cursor/gemini) with an env-tunable per-chunk timeout, so one large/slow source can't time out the whole run.
- **`ctx session list | import | ignore | unignore`.** Discover agent sessions on this machine as a stable, newest-first, **numbered** list (paged 25, with new/imported/ignored status), then import a chosen subset by number/range/`all` or via an interactive picker. Import is **key-aware** (prints a RICH-vs-PLAIN notice read from env/config — never prompts for or stores a secret) and routes each session to the workspace of the repo it ran in. `session ignore`/`unignore` maintain a **privacy skip-list** (`~/.ctxone/ignored-sessions.txt`) honoured by both manual imports and the background sweep. `session import --to <ns>` forces a workspace.
- **Graphical import in Lens.** The Sessions view gains an **Import sessions** panel: a checkable, filterable list of discovered transcripts with per-row and bulk **workspace assignment**, Import selected / Import all new, Mark private / Unmark, an **auto-sync toggle** (enable/disable the background sweep and its cadence at runtime — no restart, persisted across restarts), and a plain-language extraction-key note. New endpoints: `GET /api/sessions/discoverable`, `POST /api/sessions/import`, `POST /api/sessions/ignore|unignore`, `GET|POST /api/sessions/autosync`, and `GET /api/sessions/imported`.

### Fixed
- **`ctx doctor` no longer flags the canonical database as "stray."** The check compared raw file counts, so a `./ctxone.db` stub existing beside the real `~/.ctxone/memory.db` fingered *both* as strays. A stray is now specifically a stub that is not the canonical db; the message and suggestion name only the actual stragglers.
- **`why_did_we` returns rationale, not transcript chatter.** Session-transcript captures (full-turn / title / meta nodes under `/sessions/`) matched decision queries and buried the recorded rationale under an agent's own tool-call output. Both the HTTP and MCP paths now over-fetch and drop those paths.
- **Code-proxy 404 spam.** The Lens sidebar fired `/api/code/<repo>/health` + `/prefetch` for a repo that wasn't registered (a name persisted from another hub), 404-ing on every load; the calls are now gated on registry membership and a stale selection is cleared.
- **Live Tail stuck on "Connecting…".** A backgrounded tab skips polling, which left the feed on a permanent, misleading "Connecting…". The status is now driven by whether a poll has actually completed plus tab visibility: a hidden tab reads "paused (background)", an idle foreground feed reads "watching", and only a never-completed foreground poll reads "connecting".
- **`plan complete` vs `plan done` disambiguated in `--help`.** `plan done` now states it closes a single *task* (and points to `close`/`complete` for a whole plan); `plan complete` states it force-completes the whole *plan* (and points to `plan done` for one task).

### Changed
- **Cumulative token labels.** The workspace dashboard and per-session tiles relabel their token totals as **cumulative** ("Tokens used (cumulative)" / "tok · cumulative", with tooltips), so large running totals read as expected rather than as a single call.

## [v0.9.32] — 2026-08-11

### Added
- **`ctx ingest-session --dir-workspaces`.** Sessions imported from a working directory that isn't a git repo now route to a workspace named after the directory (kebab-cased basename) instead of the `default` namespace — grouping sessions by working directory the way Claude Code / Codex do. Opt-in: bare directory names are less canonical than a git remote (they can collide and proliferate), so it's off by default; git repos still key on their remote's `owner/repo`. This also makes a manually-moved non-git-dir session durable against a future re-sync (it routes back to its named workspace, not `default`).

## [v0.9.31] — 2026-08-11

### Added
- **Move a session to a workspace from the Lens Sessions view.** The selected session's detail header gains a `Workspace → [move to…] → Move` control that pins an imported session (one that landed in the wrong workspace, usually `default`) to the right one — a one-click UI over the guarded `move_session` endpoint. On success the list reloads and a "✓ Moved to `<ns>`" banner shows; errors render inline; the current workspace is excluded from the target list.

## [v0.9.30] — 2026-08-11

### Added
- **Safe cross-namespace move.** New `ctx move <path> --to-namespace <ns>` (with `--dry-run`, `--no-delete`, `--from-namespace`, `--to-branch`) and `POST /api/move` relocate an arbitrary subtree of graph paths to another workspace. Plus `ctx session move <id> --to <ns>` for CLI parity with `ctx plan relocate`. A new "Moving data between workspaces" section in the CLI reference documents all three and clarifies they are *not* the deny-by-default cross-namespace merge.
- **MCP namespace lookup tools.** `namespace_for_plan` and `namespace_for_task` scan every workspace to resolve which one owns a given plan/task id.
- **Lens plan scope selector.** The Plans view gains a scope control — **Plan** (one plan) → **All Plans** (every plan on the branch) → **All Branches** (every plan across the workspace) — with a filterable aggregate list (status, task counts, branch tag) and cross-branch open.
- **Glob search in Lens.** The plan switcher, task search, and aggregate list accept `*` (any run) and `?` (one char); a bare term stays a substring match.

### Fixed
- **Silent data-loss in cross-namespace moves.** `relocate_plan`/`move_session` deleted the source subtree *unconditionally* after a conditional write, so a corrupt/empty read (e.g. partial-tree corruption after many sequential deletes) or a skipped write could destroy the source. All moves now route through a single **write → verify (leaf-count) → delete** guard: the source is never deleted unless the target provably received the same leaves.
- **MCP fail-closed on an underivable namespace.** The stdio MCP server silently fell back to the `default` namespace when it couldn't derive a workspace from the cwd, mis-filing writes. Write tools now refuse with actionable guidance (reads still work; an explicit `default` is honored), and read tools annotate results with a workspace notice on a fallback-`default` session.
- **Lens Plans loading state.** Switching workspaces briefly rendered "No plans on this branch yet" during the in-flight reload; it now shows a loading placeholder.

## [v0.9.29] — 2026-08-10

### Added
- **Browse-with-search on Search, Recall, and Why.** The Lens Browse tree+detail panel is now embedded beside the results on the Search, Recall, and Why pages: clicking a result (or a Why trace path) reveals it in the in-page tree and loads its value/provenance without navigating away to `/browse`. The browser is extracted into a reusable `BrowsePane` component; `/browse` itself keeps its `?path=` deep link.

### Fixed
- `BrowsePane` no longer flashes "No memory on this branch yet" during its initial path load. The empty state was shown whenever the path list was empty — including the in-flight first fetch, which is slow because it returns the whole tree — so a cold load briefly and misleadingly claimed the branch had no memory. It now shows a "Loading memory…" placeholder until the first load completes.

## [v0.9.28] — 2026-08-07

### Changed
- Split the Lens "Token economics" panel into distinct **Recall savings** and **LLM usage** views, and broke **Model efficiency** out into its own panel.

### Fixed
- Corrected inflated cost figures: `$/1M` now excludes cache reads, and the recall savings estimate no longer double-counts.

## [v0.9.27] — 2026-08-06

### Changed
- CLI output no longer frames the per-recall flat-vs-injected ratio as "savings" — the last place still using the old "N tokens sent vs M flat (X× savings)" language. `ctx recall` now reports just the honest injected-token count, and `ctx demo`'s cumulative line reads "Estimated savings this session (conservative model … can't be measured) — N injected · ~M saved (est.)", consistent with the Lens reframing in v0.9.26.

## [v0.9.26] — 2026-08-06

### Changed
- Token savings is now presented as an honest, bounded **estimate** rather than a measurement. The old figure divided the entire serialized memory graph by each recall injection and accumulated it per call, producing implausible ~9000× ratios and billions of "saved" tokens. Savings from a memory tool is inherently a counterfactual (the avoided run never happened), so `tokens_saved` is now a conservative model — `(RECONSTRUCTION_FACTOR − 1) ×` the real injected recall payload — that grows with the session and never touches the graph. Lens tiles are relabeled "Tokens saved (est.)" and the flat-memory ratio is removed.
- Session detail leads with the efficiency (burn) verdict instead of the discredited savings ratio; a burning/diminishing session now offers a **"Roll to a new session"** action.

### Added
- `GET /api/sessions/{sid}/starter` — generate a fresh-session seed from the user's own words (verbatim `user_text`, de-noised, grouped by topic arc). Pure, no-LLM. Surfaced in Lens as the payload the "roll to a new session" CTA copies.
- `SessionSnapshot.session_startup_tokens` — the first-recall payload, surfaced as the session's startup boost.

## [v0.9.20] — 2026-07-29

### Added
- `ctx branch reset <name> --to <ref> [--backup]` and `POST /api/branches/reset` — reset a branch to a ref, optionally snapshotting a timestamped recovery ref first.
- `ctx merge --dry-run` / `--allow-deletions` across CLI, HTTP, and MCP — preview a merge and gate merges that would delete nodes.
- `ctx init --no-mcp` — skip the MCP config step during init; AGENTS.md is primed non-interactively (no Y/n prompt).

### Changed
- Merge now enforces a plans-domain policy: rejects done→pending task regressions, promotes active→completed plans post-merge, and preserves task proofs.

### Fixed
- `archive_plan` resolves the branch from the query string then the JSON body, fixing spurious 404s when archiving plans on non-default branches.

## [v0.9.19] — 2026-07-29

### Added
- `ctx db upgrade [--check]` — run pending schema migrations behind a snapshot and a post-upgrade `fsck` integrity check.

### Changed
- Plan/task/doc read helpers (`plan next` in-progress, `plan stale`, `docs find`) now surface repository-integrity errors instead of masking a corrupt tree as an empty result.
- CHANGELOG version headers no longer carry a `v` prefix, matching AgentStateGraph and AgentStateDeveloper.

## [v0.9.16] — 2026-07-21

A large release centred on **workspace-scoped sessions**, **more import
sources**, **ingest performance**, and a **session-efficiency ("burn") board**
in Lens. Summarised from the commit history.

### Added — sessions & workspaces

- Each transcript is routed to its own **workspace**; existing sessions are
  migrated out of `default` into their workspaces, and the session list is
  scoped accordingly.
- Sessions can be deleted and stay deleted.
- Per-turn **git provenance** captured and rolled up per session.
- Plan/task links derived server-side; `--namespace` honoured explicitly.
- A session's **burn score** is persisted at ingest and read by the dashboard.

### Added — import sources

- **Gemini and Cursor** session ingestion, extending the `SessionSource` seam.
- `--since` skips whole old sessions; `--scan-dir` imports from custom dirs.

### Added — Lens

- A **burn board** ranking the least-efficient sessions: scans every session,
  ranks the worst, caches per workspace+branch, with a time-range column and a
  scrollable list.
- A "Recently updated" dashboard panel.
- Independent scrolling for the session list and detail panes; the app shell is
  pinned so the document no longer scrolls.

### Changed — performance

- SQLite opened in **WAL mode** (`synchronous=NORMAL`); `agentstategraph` bumped
  to v0.9.4. ~1.75× faster cold import.
- Ingest: concurrent sessions, bulk turn writes with aggregated token records,
  index-only Cursor discovery, and skipping unchanged sessions on re-sync.

### Fixed

- `--file` always imports, ignoring a remembered `--since` window.
- Request body limit raised so large bulk turn writes don't 413.
- The web unit suite is runnable again and runs in CI; `npm ci` unbroken.

## [v0.9.15] — 2026-07-19

Two themes: the first step of **multi-agent session import** (Claude Code is no
longer the only source), and a round of **Lens fixes** for figures that were
quietly misleading.

### Added — multi-source ingestion

- A `SessionSource` seam splits "where transcripts live" from "what we do with
  them". Claude Code moves behind it unchanged; `metrics.rs` no longer carries a
  duplicate copy of the directory walk.
- **Codex adapter** — imports `rollout-*.jsonl` from `~/.codex/sessions` and
  `~/.codex/archived_sessions`. Verified against 195 sessions / 5,865 turns.
- `ctx ingest-session --source claude|codex|all`. An unknown source is an error
  listing the known ids, not a silent empty result.
- Session ids are namespaced by source (`codex:<uuid>`). Claude Code is exempt:
  its ids are already keyed into existing rows, memory tags and turn paths.
- Sessions now carry the real agent label in `source` instead of a hardcoded
  "Claude Code", and the reported provider is derived from the model rather than
  always being `anthropic`.

### Added — source-native token classes

- `extra_tokens` preserves token classes the four normalised counters cannot
  express — Codex `reasoning_output_tokens`, Gemini `thoughts`/`tool` — verbatim
  under the reporting agent's own field names, persisted in a new sparse column.
  Folding them into `output_tokens` would have destroyed the distinction and made
  cross-agent comparison wrong.
- `GET /api/stats/activity/{ref}?days=N` — per-day commit counts for the
  activity heatmap, aggregated server-side.

### Fixed

- **Session stats were lost on SIGTERM.** The shutdown handler awaited `ctrl_c`
  alone despite its comment; `launchctl bootout`, `docker stop`, a reboot and a
  plain `kill` all skipped the final flush, dropping up to 30s of usage on every
  service stop.
- **Auto-refresh blanked the page.** Pages guarded on a bare `loading` flag
  replaced their content with a skeleton every poll, losing scroll position and
  any open panel. Refresh cadence also widened 15s → 30s.
- **The activity heatmap charted a commit-count window, not a time window.** It
  bucketed the 1000 most recent commits, so a busy machine saw *less* history —
  at one point 80 minutes. Now a fixed, zero-filled day window.

### Changed — Lens

- Session commits are colour-coded by conventional-commit type with an
  orphan/linked accent, plus type and linkage filters.
- New **token usage over time** charts, total and split by model.
- LLM consumption figures render compact (5.6B) with the exact value on hover.
- **Every dashboard panel now states its scope** — `all branches` for global
  token figures, the branch name for branch-scoped ones. The dashboard mixes
  both, and an empty Activity panel previously read as "collection stopped"
  rather than "this branch finished in May".

### Known gaps

- Cursor, Gemini CLI and Android Studio adapters are designed but not built.
- Memory-op savings still pool into the `default` session: MCP-over-HTTP does not
  resolve a session id, so per-session `tokens used`/`saved` read 0 for sessions
  created since HTTP became the default transport.
- The heatmap intensity ramp is linear against the maximum, so one very busy day
  flattens the rest of the range.

## [v0.9.14] — 2026-07-18

The headline work since v0.9.13: project **namespaces** and the **unified
MCP-over-HTTP daemon** that makes one process serve MCP, the REST API, and the
Lens web UI.

### Added — namespaces & projects

- Project **namespaces** isolate everything ref-scoped (branches, plans, memory,
  taints, reminders, history); one namespace typically maps to one code repo.
- `ctx project add/list/use/detect` plus a `POST /api/projects` registry;
  namespace threaded through every ref-touching endpoint (query param,
  `X-CTXone-Namespace` header, else `default`).
- Per-session namespace resolution in MCP + a `project_status` tool.
- Git-branch mirroring **within** a project namespace.
- `ctx plan list --all-namespaces` for a global plan inventory.

### Added — unified MCP-over-HTTP daemon (now the standard setup)

- **MCP over HTTP** (Streamable HTTP) at `POST /mcp?namespace=<ns>`: one
  `ctxone-hub --http` process serves MCP + REST + Lens. Removes the
  stdio startup-order / db-lock race.
- `ctx init` now defaults to `--transport http`, writing URL-based configs
  (native `http` for Claude Code/Cursor/VS Code/Codex; an `mcp-remote` stdio
  bridge for Claude Desktop). Preflights the daemon; stdio remains a fallback.
- `ctx service install/uninstall/status` — run the daemon as an always-on
  login/boot service (launchd, systemd, or Windows Task Scheduler).

### Added — security posture

- Optional **bearer-token auth** over REST + `/mcp` (`--auth-token` /
  `CTXONE_AUTH_TOKEN`): non-loopback requests need `Authorization: Bearer`;
  loopback is exempt. Tokens are injected into `ctx init --transport http` configs.
- **Origin guard + tightened CORS** (CSRF / DNS-rebinding): same-origin always
  passes; foreign origins rejected unless allow-listed (`--allowed-origin` /
  `CTXONE_ALLOWED_ORIGINS`). Lens is local/tunnel-only over an authed hub.

### Added — plans, data & docs

- `ctx plan link` — a task can satisfy a task in another plan (cross-plan);
  `ctx plan stale` — surface in-progress tasks idle for N days;
  `ctx plan next --in-order` + in-progress tasks shown separately;
  non-blocking warning when starting a task while another is in progress;
  richer proof-parse errors with per-kind examples.
- `ctx db export` / `ctx db import` — portable JSON branch-graph snapshots.
- `ctx docs` registry — index canonical docs; `import-doc` alias for `prime`.
- `ctx reminder tick` — executor for due reminders: runs a reminder only if it
  is explicitly **`autonomous: true`** **and** every one of its commands is on an
  exact-match allowlist (`~/.ctxone/reminder-tick.allow`); records the outcome
  and snoozes anything unapproved. (Gating on the `autonomous` flag directly,
  not just status, is fail-closed and sidesteps an `agentstategraph-reminders`
  bug where `remind_me` can promote a non-autonomous reminder straight to `due`.)
- `ctx service tick install/uninstall/status` — install the tick on a periodic
  timer (launchd `StartInterval`, a systemd `.timer` + oneshot `.service`, or a
  Task Scheduler repetition trigger). `--interval`, `--allowlist`, `--skip`.
- New MCP tools `plan_link`, `plan_stale`, `docs_find`.

### Added — Lens

- Namespace switcher + `/projects` page; reminders, recall, why-did-we, and
  live-tail pages; `/code/*` renders from `@agentstate/lens-core`; streaming
  (SSE) passthrough for the ASD proxy.

### Changed

- Agent-discoverability polish across the CLI help, MCP tool descriptions, and
  the reference docs (`AGENTS.md` task-discipline + docs-model rules).
- **Reminders now fail closed**: the `autonomous` flag defaults to `false`
  (was `true`). A reminder created without an explicit `autonomous: true`
  surfaces as `awaiting_permission` and must be `reminder_approve`d before any
  executor acts on it — safer now that reminders can carry commands an executor
  will run. Set `autonomous: true` explicitly for unattended reminders.

## v0.9.13 — 2026-06-08

### Fixed

- `release.yml` builds the SvelteKit frontend before `cargo` so Lens assets
  ship in the release binary.

## v0.9.12 — 2026-06-08

### Added

- GitHub Actions release pipeline (first automated release) + Windows
  distribution; `scripts/release.sh` + `RELEASE.md`.
- Lens surfaces Plan G/K thinking from `asd-serve`; sidebar split into CtxOne
  and ASD sections.

## v0.9.11 — 2026-06-03

### Added — asd-multi-repo integration

- Shared registry at `~/.config/asd/repos.toml` consumed by the `asd` CLI,
  `asd-mcp`, and CTXone Hub. Auto-discovered by the hub at startup when no
  `--asd-url` / `--asd-path` flags are given.
- `AsdProcessPool` in the hub: lazy-spawns `asd-serve` per registered repo,
  evicts idle children. Flags `--asd-path name=/path/db` and
  `--asd-idle-timeout <secs>`.
- MCP tools `set_active_repo(repo)` and `get_active_repo()` with
  per-session active-repo resolution: explicit > session > single registered.
- Lens repo picker now shows hot/cold status dots and fires a prefetch on
  selection.
- New `POST /api/code/{repo}/prefetch` endpoint warms a pool entry.
- New `/code/thinking` page surfaces Plan G/K thinking captured by asd
  (hypotheses, mental models, open questions, failed attempts), with a
  confidence-floor slider. Symbol detail page grows an "Inherited thinking"
  panel.
- End-to-end harness at `scripts/e2e/asd-multi-repo.sh`.

### Changed — Homebrew tap migration

The Homebrew tap moved from `ctxone/tap` (GitHub `ctxone/homebrew-tap`,
now archived) to `agentstatelabs/ctxone` (GitHub
`agentstatelabs/homebrew-ctxone`, mirrored from
`git.internal.example/agentstategroup/homebrew-ctxone`). Release
tarballs likewise moved from `ctxone/ctxone-docs/releases` to
`agentstatelabs/ctxone-releases/releases`.

Existing users on the old tap should run:

```sh
brew untap ctxone/tap        # untap first if you've already migrated install source
brew tap agentstatelabs/ctxone
brew reinstall ctxone        # picks up the new release URL
```

(`brew untap` refuses while a formula from that tap is still installed —
do the `reinstall agentstatelabs/ctxone/ctxone` first if needed.)

### Changed — UI

- Lens sidebar split into "CtxOne" and "ASD" sections. Repo picker moves
  under ASD; branch picker stays under CtxOne.
- Plans page header reorganised into two rows.
- Dev workflow: `npm run dev` proxies `/api/*` to a locally-running hub
  (override target via `VITE_HUB_URL`).

### Fixed

- Empty-repo race on first render of `/code` — `selectedRepo` now
  hydrates synchronously from localStorage at module import.
- `AsdProcessPool` correctly parses the resolved bind address from
  `asd-serve` stdout (asd-serve 1.0.81+) and gates on `/api/v1/health`.

## v0.9.10 — 2026-05-13

- Rebased versioning to the 0.9.x series after the 0.8x experiment.
- See commit `274f679` for the full diff.

## Prior history

For releases before v0.9.10 see `git log` and the archived GitHub
repos under `ctxone/` (now archived; canonical history continues on
`git.internal.example/agentstategroup/ctxone`).

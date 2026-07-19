# Changelog

All notable changes to CTXone are documented here. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning follows
the project's `0.9.x` series (incremented by 0.0.01 per release).

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

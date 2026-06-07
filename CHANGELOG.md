# Changelog

All notable changes to CTXone are documented here. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning follows
the project's `0.9.x` series (incremented by 0.0.01 per release).

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

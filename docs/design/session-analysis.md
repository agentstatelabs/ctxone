# Session Analysis Subsystem

Status: in progress · Plan: `session-analysis-subsystem`

## Goal

Turn every agent session — scraped locally **or** imported — into structured,
searchable, shareable knowledge in a CTX hub, across devices and clients.

Analysis produces, per session and per **topic arc** within it:

1. **Memory extraction** — durable facts → `remember` (exists today).
2. **Summary** — session-level and per-arc.
3. **Topic segmentation** — arcs, extended beyond branch/cwd/idle to topic shifts.
4. **Pseudo-branch structure** — topic shifts become pseudo-branches for clients
   that don't branch natively. ThreadWeaver branches natively; Claude/Codex/etc.
   get synthesized pseudo-branches so everything is uniformly branchable and
   searchable.

## Two orthogonal halves

- **Capture is a source seam.** Sessions arrive by (a) the **local scraper**
  (hub-side sweep — Claude/Codex/Cursor/Gemini/ThreadWeaver-local) or (b) an
  **import seam** (file/paste/API upload, later per-provider connectors) for
  sources with no local artifacts (e.g. Copilot is server-side).
- **Analysis is source-agnostic.** Everything lands as source-neutral `Turn`s,
  so extract/summarize/segment/pseudo-branch run identically no matter how the
  session arrived.

## Pluggable extraction provider

`provider ∈ {anthropic, openai, gemini, openai-compatible}` + `model` + (for
openai-compatible) `base_url`. The extraction prompt is provider-neutral (emits
JSON memories); each provider has its own request/response shape and auth.

Config: `~/.ctxone/extraction.toml` (`provider`, `model`, `base_url`, optional
`key_file`). Keys resolve from env (`ANTHROPIC_API_KEY` / `OPENAI_API_KEY` /
`GEMINI_API_KEY`) or a `600`-perm `~/.ctxone/keys/<provider>`.

## Analysis status: hub-stored

Per-session record in the hub (authoritative; CLI + Lens + cross-device all read
it): `analyzed_through` (turn idx), `analyzed_at`, `provider`, `model`,
`input/output tokens`, `est_cost`. The local watermark
(`~/.ctxone/extraction-watermarks.json`) is retained only as the sweep's
fast-path cache.

## Cost estimation before spend

Any analyze run first computes `N sessions, M substantial un-analyzed turns,
~X input tokens → est $Y` from a per-model price table and shows it. Nothing
spends until confirmed. This makes pointing at gigabytes of history safe.

## Rollout policies (fall out of the above)

- **A — going-forward:** seed each session's watermark to its current turn count.
- **B — full backfill:** `analyze run --unanalyzed`.
- **Selective:** `analyze run --session <id>…`.

All gated by the cost estimate.

## Surfaces

- **CLI:** `ctx analyze list` (sessions + status); `ctx analyze run
  [--all | --unanalyzed | --session <id>…] [--provider --model] [--dry-run]`.
- **Lens:** analysis-status column + filter on Sessions, multi-select, "Analyze
  selected" with cost estimate + progress.

## Control layer

Same switches, two scopes.

**User (shipped):**

- **Disable scraping** — set the hub sweep interval to 0:
  `CTXONE_SESSION_SYNC_INTERVAL_SECS=0` (no transcripts are pulled).
- **Disable analysis** — `enabled = false` in `~/.ctxone/extraction.toml`
  turns extraction/analysis off even when a key is present
  (`resolve_extraction_config` returns `None`). Analysis is also implicitly off
  until a key is configured, so it **never silently spends**.
- **Provider/model choice** — `~/.ctxone/extraction.toml`.

**User (designed, not yet shipped):** per-source / per-project / per-session
granularity and spend caps.

**Team/Enterprise (designed):** the same switches as **centrally-managed,
RBAC-scoped policy** via `agentstategraph-policy` (ties into the enterprise
policy-governed rollout): admins enforce/forbid scraping+analysis, restrict
allowed providers, set org spend caps and data-residency — enforced at the hub,
not per-machine.

Example `~/.ctxone/extraction.toml`:

```toml
enabled  = true            # false disables analysis entirely
provider = "anthropic"     # anthropic | openai | gemini | openai-compatible
model    = "claude-haiku-4-5-20251001"
# base_url = "https://…"   # openai-compatible only
# key_file = "~/.ctxone/keys/anthropic"
```

## Phases

| # | Phase |
|---|-------|
| P1 | Provider abstraction + keys/config |
| P2 | Hub-stored analysis status (migrate watermark) |
| P3 | Cost estimation + price table |
| P4 | Analysis pipeline: summary + topic segmentation + pseudo-branch |
| P5 | Import seam (file/paste/API) for non-local sources |
| P6 | CLI `ctx analyze` |
| P7 | Lens analyze view |
| P8 | Control layer (user toggles → Enterprise policy) |

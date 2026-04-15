# CTXone for Open WebUI

CTXone ships two Open WebUI plugins out of the box:

- **Tool** — function-calling tools the model can invoke explicitly
  (`remember`, `recall`, `forget`, `list_pinned`). Same capability as the
  MCP server, but with per-user Valves and automatic agent IDs.
- **Filter** — an in-process hook that runs around every chat turn.
  `inlet` calls `recall` on the user's message and injects the result
  as a system prompt; `outlet` optionally captures the assistant's
  reply as a fact. This works with models that don't support
  tool-calling at all.

Both live in the same file —
`bindings/python/src/ctxone/integrations/openwebui.py` — and share a
Hub client. You can use either or both.

## Which one do I want?

| Goal | Use |
|---|---|
| Let the model decide when to save/load facts | **Tool** |
| Automatically inject relevant memory on every turn | **Filter** |
| Make memory work with non-tool-calling models (Llama 2, old Mistral) | **Filter** |
| Capture the assistant's answers as future facts | **Filter** (opt-in) |
| Give users per-account private memory branches | **Either** (both support `UserValves.branch`) |

Most users end up installing both. The Filter runs on every turn so
the model sees relevant memory automatically; the Tool is there for
when the model *knows* it wants to save something important, like
"remember that we picked BSL-1.1".

## Install

You need a running CTXone Hub somewhere Open WebUI can reach. The
[Quickstart](QUICKSTART.md) covers that — easiest is
`docker compose up` from the repo root, which exposes the Hub on
`http://localhost:3001`.

### Option 1: Paste into Open WebUI (fastest)

1. In Open WebUI: **Admin Panel → Functions → +**
2. Paste the entire contents of
   [`bindings/python/src/ctxone/integrations/openwebui.py`](../bindings/python/src/ctxone/integrations/openwebui.py)
3. Save. Open WebUI reads the `requirements: ctxone>=0.73.0` line in
   the docstring frontmatter and auto-installs the client into its
   Python environment (make sure
   `ENABLE_PIP_INSTALL_FRONTMATTER_REQUIREMENTS=true` is set in your
   Open WebUI env — it usually is for self-hosted installs).
4. The file defines both `Tools` and `Filter` classes. Open WebUI
   registers both; enable the ones you want from the Functions list.

### Option 2: Install as a Python package

If you run Open WebUI from source or in a venv you control:

```bash
pip install "ctxone[openwebui]"
```

Then in your own plugin file:

```python
from ctxone.integrations.openwebui import Tools, Filter
```

and save it as an Open WebUI function. This is cleaner if you want to
pin the ctxone version yourself or ship it in an image.

## Configuration (Valves)

Both plugins expose two tiers of Pydantic config Open WebUI renders
as forms:

### Admin Valves (one per install)

| Field | Default | What it does |
|---|---|---|
| `hub_url` | `http://localhost:3001` | Where to reach the Hub |
| `default_branch` | `main` | Branch for reads and writes when users haven't picked their own |
| `timeout_seconds` | `15` (Tool) / `8` (Filter) | HTTP timeout. The Filter uses a shorter timeout because it's in the hot path of every turn |
| `allow_writes` | `true` (Tool only) | Set false for read-only demos |
| `recall_budget` | `1500` | Default token budget passed to `recall()` |
| `min_query_length` | `3` (Filter only) | Skip recall when the user message is shorter than this |
| `silent_on_error` | `true` (Filter only) | If true, Hub errors are swallowed and the chat continues; if false, they fail the turn |
| `priority` | `-10` (Filter only) | Open WebUI filter priority. Lower runs earlier — memory injection should happen before most other filters |

### UserValves (per-user overrides)

Each user in Open WebUI gets their own copy of these:

| Field | Default | What it does |
|---|---|---|
| `enabled` | `true` (Filter only) | Master switch — turn off if you don't want memory injected into your chats |
| `branch` | `""` (empty) | Your private branch. Empty means "use the admin default" |
| `remember_importance` | `medium` (Tool only) | Importance level this user writes facts with |
| `capture_replies` | `false` (Filter only) | If on, store the assistant's reply as a fact on every turn |
| `capture_importance` | `low` (Filter only) | Importance applied to auto-captured replies |

## How attribution works

Every commit CTXone writes needs an agent ID — that's the "who"
shown in `ctx blame` output. The integration resolves this from
Open WebUI's `__user__` dict, in this order:

1. `user.email` (preferred — typically the login email)
2. `user.name` (display name)
3. `user.id` (UUID)
4. `"openwebui"` (fallback if none of the above)

So a `remember` call from alice@example.com shows up in
`ctx blame /memory/facts/abc` as `alice@example.com` with the exact
timestamp, intent, and reasoning the Hub recorded. Your team gets
real provenance for free.

## How the Filter feels in practice

The user types a question. Before it hits the model, the Filter
runs:

1. Extract the last user message (handles multimodal content by
   joining the `text` parts and ignoring images).
2. Skip if shorter than `min_query_length` or if the user has the
   filter disabled.
3. Call `hub.recall(topic=last_message, budget=recall_budget)`
   against the user's private branch.
4. If there are matches, render them as a system prompt block:

   ```
   ## Relevant memory from CTXone
   Retrieved for topic: 'licensing'

   - [pinned] **Vision** — ship a BSL-1.1 product with MIT clients
   - [fact] CTXone uses BSL-1.1
   - [fact] Converts to Apache 2 after 4 years

   _(CTXone: this retrieval is 14.2× smaller than loading the full
   memory graph.)_
   ```

5. Prepend this as a **new** system message. It never overwrites
   whatever system prompt the model already has.
6. Return the mutated body. The model now sees the memory before
   generating.

If `capture_replies` is on, the Filter also watches the outlet. When
the full assistant response arrives, it calls `hub.remember` with the
reply as a low-importance fact under `/memory/openwebui/<user>/`.
This makes every conversation self-teaching — but it can also fill
your memory with junk, which is why it's off by default.

## Troubleshooting

### "CTXone Hub is unreachable"

The most common cause is Open WebUI running in Docker with the Hub
running on the host. `localhost:3001` inside a container refers to
the container itself. Change `hub_url` to either:

- `http://host.docker.internal:3001` (macOS, Windows, recent Linux)
- `http://172.17.0.1:3001` (Linux default Docker bridge)
- Or run both in the same `docker compose` network — see
  [`docker-compose.yml`](../docker-compose.yml) for a working setup.

### "The Filter takes too long, chat feels slow"

The Filter calls the Hub synchronously in `inlet`. On a fast local
Hub this is 5–20 ms, which is invisible. If it's slow:

1. Drop `timeout_seconds` to 3 or 4. The Filter swallows timeouts
   when `silent_on_error=true`, so a slow Hub just means no memory
   injection on that turn.
2. Lower `recall_budget` — smaller budgets return faster because the
   Hub prunes earlier.
3. Check `RUST_LOG=info` on the Hub and look at the `recall` lines.
   Each one prints tokens sent and the savings ratio.

### "I don't want the Filter on every chat"

- `self.toggle = True` in the Filter's `__init__` makes Open WebUI
  render a per-chat switch in the UI. Click it off and the Filter
  skips that conversation.
- Or set `UserValves.enabled = false` to turn it off permanently
  for your account.

### "How do I see what was auto-captured?"

```bash
ctx ls /memory/openwebui/
ctx --branch <your-branch> recall "<topic>"
```

Or browse through CTXone Lens at `http://localhost:5173/browse` — the
blame panel shows which user wrote each fact via Open WebUI and when.

## Related docs

- [QUICKSTART.md](QUICKSTART.md) — spinning up a Hub
- [INTEGRATIONS.md](INTEGRATIONS.md) — MCP wiring for Claude Code, Cursor, etc.
- [VISION.md](VISION.md) — why CTXone exists at all

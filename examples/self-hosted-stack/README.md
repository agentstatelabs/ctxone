# Self-hosted CtxOne stack

A turnkey self-hosted AI environment — memory, web UI, local LLM, and
chat — wired together in a single `docker compose up`. This example
exists to answer the question: *"What does a real CtxOne deployment
actually look like on my own hardware?"*

## What you get

| Service | Port | What it does |
|---|---|---|
| `ctxone-hub` | 3001 | Memory layer — MCP + HTTP API |
| `ctxone-lens` | 5173 | Web UI for browsing, blame, diff, forget |
| `ollama` | 11434 | Local LLM runtime |
| `open-webui` | 8080 | Chat UI with the CtxOne plugin pre-mounted |
| `seed` | — | One-shot container that primes the Hub from `./prime/*.md` |
| `threadweaver` | (commented out) | Optional — uncomment once your image is ready |

All services share a Docker network called `ctxone`, so inside the
stack they reach each other by service name (`http://hub:3001`,
`http://ollama:11434`, etc).

## Quick start

```bash
cd examples/self-hosted-stack
cp .env.example .env
# edit .env to set WEBUI_SECRET_KEY and any port overrides
docker compose up -d
```

First start takes a minute or two while Open WebUI initializes. Check
health:

```bash
docker compose ps
curl http://localhost:3001/api/health
```

Then open:

- **Chat**: http://localhost:8080 (Open WebUI — create an admin account on first visit)
- **Memory browser**: http://localhost:5173 (CtxOne Lens)
- **Hub API**: http://localhost:3001/api/health

## First run: pull a model

Open WebUI needs a model. From your host:

```bash
docker exec -it ollama ollama pull llama3.2
```

Or from inside Open WebUI: click the model selector → "Manage
Models" → paste a name. Small models work fine for testing —
`llama3.2:1b`, `phi3:mini`, `qwen2.5:0.5b`.

## Wire up the CtxOne Open WebUI plugin

The stack mounts
`bindings/python/src/ctxone/integrations/openwebui.py` into the
`open-webui` container at `/app/ctxone-plugin.py` so you don't have to
copy-paste. Two ways to install it:

### Option A — Paste into the admin UI (recommended)

1. http://localhost:8080 → **Admin Panel** (gear icon) → **Functions** → **+**
2. Paste the contents of
   [`../../bindings/python/src/ctxone/integrations/openwebui.py`](../../bindings/python/src/ctxone/integrations/openwebui.py)
3. Save. Open WebUI reads the `requirements: ctxone>=0.70.0` line in
   the docstring and auto-pip-installs the client
   (`ENABLE_PIP_INSTALL_FRONTMATTER_REQUIREMENTS=true` is already
   set in the compose file).
4. The file registers **both** `Tools` and `Filter`. Toggle on the
   ones you want from the Functions list.
5. Open the **Valves** for each and set `hub_url` to `http://hub:3001`
   (the in-network hostname, since Open WebUI's Python runtime
   reaches the Hub from inside the Docker network).

### Option B — Import from filesystem

Open WebUI can load Python files mounted inside the container. The
compose file already mounts the plugin at `/app/ctxone-plugin.py` —
point Open WebUI's function loader there if you prefer a
version-pinned import over paste-and-edit.

## Seed content

The stack runs a one-shot `seed` container on every `up` that primes
the Hub from any `*.md` file in [`./prime/`](./prime/). The files
here get parsed at H1/H2 headings and stored as pinned memories
under `/memory/pinned/<filename>/*`, so every `recall` will include
them regardless of topic.

To add your own:

```bash
cp my-project-README.md prime/
docker compose up seed
```

Re-running is idempotent — `ctx prime` with the same source name
overwrites the previous entries.

## Using it

Open Open WebUI, start a new chat, and ask a question about
something in the primed content (say, "what's our licensing
default?"). Because the CtxOne **Filter** is enabled, the Hub is
hit automatically on every turn:

1. The Filter intercepts your message in `inlet()`.
2. It calls `hub.recall("what's our licensing default?")`.
3. The result gets prepended as a system message.
4. The model sees the memory before generating.

You should see the model answer correctly even on a brand-new chat,
because the pinned onboarding notes are always in context.

To verify what's happening:

```bash
# Watch the Hub logs — every recall prints its topic, tokens sent,
# and savings ratio.
docker compose logs -f hub

# Browse what's stored.
open http://localhost:5173/browse
```

## Adding ThreadWeaver

The `threadweaver` service in `docker-compose.yml` is commented out.
Uncomment it and replace the image name with your actual
ThreadWeaver image once you have one. Until ThreadWeaver ships
native MCP client support, the integration pattern is:

- Point TW's runtime at `http://hub:3001` as a memory backend.
- Wrap the HTTP API calls in a small adapter module (equivalent to
  the Python `Hub` client at `bindings/python/src/ctxone/client.py`).
- Use TW's own hook system — not the Open WebUI plugin format — to
  drive `inlet`/`outlet`-style memory injection. See the TW design
  doc for why this is the right boundary.

Once TW has an MCP client, a cleaner alternative is:

1. Drop `ctxone-hub` in stdio mode next to TW.
2. Configure TW's MCP client to spawn it.
3. All six memory tools (`remember`, `recall`, `forget`, `prime`,
   `summarize_session`, `why_did_we`) become available without any
   adapter code.

## Persistence

All state lives in named Docker volumes:

| Volume | What it holds |
|---|---|
| `hub-data` | The CtxOne SQLite database |
| `ollama-data` | Pulled Ollama models |
| `open-webui-data` | Open WebUI user accounts, chats, settings |

Survive restarts, container rebuilds, and `docker compose down`. To
wipe everything and start over:

```bash
docker compose down -v
```

To back up just the memory:

```bash
docker compose exec hub ctx export /tmp/backup.json
docker cp ctxone-hub:/tmp/backup.json ./backup.json
```

## Networking notes

- `CTXONE_API_URL_PUBLIC` in `.env` is what the **browser** uses to
  reach the Hub. It must be a URL you can hit from your laptop —
  not an in-network hostname — because Lens is client-side and
  fetches happen in your browser, not in the container.
- `hub_url` in the CtxOne Open WebUI plugin should be
  `http://hub:3001` (the in-network hostname), because Open WebUI's
  Python runtime reaches the Hub from inside the Docker network.
- Behind a reverse proxy: terminate TLS at the proxy, point it at
  `hub:3001` for the API and `lens:3000` for the UI, and set
  `CTXONE_API_URL_PUBLIC=https://your-domain.example.com`.

## Troubleshooting

### "Hub unreachable" from Open WebUI

Check the plugin's `hub_url` valve. Inside the Docker network it has
to be `http://hub:3001` (not `localhost:3001`). The compose file
sets the `CTXONE_HUB_URL` env var to help with this, but the
plugin's Valves take precedence.

### Seed container exits with "no such file"

You have no `.md` files in `./prime/`. Add at least one and re-run
`docker compose up seed`.

### Model calls are slow

Ollama is CPU-only in this compose. For GPU passthrough on Linux,
add:

```yaml
ollama:
  deploy:
    resources:
      reservations:
        devices:
          - driver: nvidia
            count: all
            capabilities: [gpu]
```

On macOS, use small models (`llama3.2:1b`, `phi3:mini`) or run the
LLM outside Docker on the host and point Open WebUI at
`http://host.docker.internal:11434`.

### Filter makes every chat feel slow

Drop the Filter's `timeout_seconds` to 3 or 4 in its Valves. The
Filter swallows timeouts silently by default, so a slow Hub turn
just means no memory on that message — not a failed chat.

## Related docs

- [docs/OPENWEBUI.md](../../docs/OPENWEBUI.md) — full Open WebUI plugin reference
- [docs/QUICKSTART.md](../../docs/QUICKSTART.md) — minimal Hub-only setup
- [docs/ARCHITECTURE.md](../../docs/ARCHITECTURE.md) — how the pieces fit together
- [docs/TOKEN_SAVINGS.md](../../docs/TOKEN_SAVINGS.md) — the math behind the savings ratio

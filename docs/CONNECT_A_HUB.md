# Connect to a Hub on another machine

A Hub usually runs on the machine your agents run on. You can point several
machines at one Hub instead: a Hub on a server of your own, one a team runs, or
a multi-tenant Hub. Every machine then reads and writes the same memory, and
Lens shows it all in one place.

Hubs don't sync with each other yet. A fact remembered through one Hub is
invisible through another, so connecting your machines to one Hub is how your
memory follows you between them for now. Sync, where each machine keeps its own
Hub and pushes to and pulls from a shared one, is designed in
[design/HUB_SYNC.md](design/HUB_SYNC.md) and not built yet.

## Before you start

- **The Hub needs a token and HTTPS.** Whoever runs it sets a bearer token and
  puts TLS in front of it ([DEPLOYMENT.md: Authentication](DEPLOYMENT.md#authentication)
  and [HTTPS in front](DEPLOYMENT.md#https-in-front)). Each person needs their
  own token on a multi-tenant Hub.
- **Check what you'd bring with you.** Nothing on this machine's own Hub moves
  when you switch, but if you'll copy it over (see
  [Bring your existing memory](#bring-your-existing-memory)), run
  `ctx secrets scan --all` against it first, and forget what it finds.

## Point a machine at the Hub

```bash
export CTX_SERVER=https://hub.example.com
export CTX_TOKEN=<your token>
ctx init --mcp-url https://hub.example.com/mcp --auth-token-env CTX_TOKEN
ctx doctor
```

- **The `ctx` CLI** reads `CTX_SERVER` and `CTX_TOKEN`. To keep the address
  without the environment, `ctx config set server https://hub.example.com`.
  The token has no config key: keep it in your shell's environment or a
  secrets manager.
- **AI tools:** `ctx init` points each detected tool at the Hub's `/mcp`, with
  the token read from `CTX_TOKEN` when the tool runs. Claude Desktop's bridge
  can't read a variable, so it needs `--auth-token <token>`, which is written
  into its config in plain text.
- **Lens:** open the Hub's address in a browser and paste the token once. Lens
  keeps it in that browser; *Forget token* in the sidebar removes it.
- **`ctx doctor`** checks that the Hub answers with your token, which
  workspace this directory resolves to, and that each tool has its entry. Its
  checks for a local Hub, such as finding the `ctxone-hub` binary, don't
  matter on a machine that only connects.

Each tool's entry names the workspace `ctx init` detected where you ran it
([DEPLOYMENT.md: topology B](DEPLOYMENT.md#b-shared-http-daemon--the-default--standard)).

## Which workspace a directory uses

Nothing changes from a local Hub. `ctx` reads the directory's `.ctxproject`
file or git remote itself and asks the Hub which workspace it belongs to, so
the Hub never needs to see your disk. A repo registered once (`ctx project add`)
resolves on every machine that has a clone with the same remote: project ids
come from the whole remote (`host/owner/repo`), and an ssh clone matches one
registered over https. A repo with no remote needs its `.ctxproject` file
committed, or `--namespace`.

## What goes to the Hub, and what stays here

| | Where it is |
|---|---|
| Memory you `remember`, `prime` or `forget`, plans, reminders and taints | On the Hub |
| Your personal namespace: agent rules from `AGENTS.md`, `remember --personal`, the project registry's records | On the Hub, so every machine that uses it shares them |
| A session's title, metadata, provenance (with this install's device id), per-model token usage and extracted memories | Sent to the Hub by `ctx ingest-session`, `ctx session import` and the Stop hook |
| A session's raw turns | **Kept here**, unless you run `ctx config set remote_turns true` (or set `CTX_REMOTE_TURNS=1` for one command) |
| The memory already on this machine's own Hub | Stays where it is; nothing copies it |
| Code intelligence (AgentStateDeveloper) | Runs where the code is. A multi-tenant Hub turns its code proxy off |
| `~/.ctxone`: config, device id, keys, backups | Here |

Usage still adds up on the Hub without turns: `ctx` works out each session's
tokens by model from the transcript on this machine and sends the totals.

## Bring your existing memory

Copying a workspace from this machine's Hub to the shared one is an export and
an import. Do it per workspace, and scan first:

```bash
# Against this machine's Hub
CTX_SERVER=http://localhost:3001 ctx secrets scan --all
CTX_SERVER=http://localhost:3001 ctx --namespace <workspace> db export --out ws.json

# Against the shared Hub (CTX_SERVER and CTX_TOKEN set as above)
ctx project add <id>            # once, from a clone of the repo
ctx --namespace <workspace> db import ws.json
```

`db import` merges: paths in the file overwrite, and paths only on the Hub are
left alone. `ctx db export` exports one branch (`--branch` picks it), and
history doesn't come with it.

## When the Hub can't be reached

There's no offline mode before sync. `ctx` commands stop with the Hub
unreachable (exit 69), agents' tool calls fail until it's back, and nothing is
queued to send later. Sessions recorded meanwhile can be sent afterwards with
`ctx ingest-session`, since their transcripts are still on this machine.

## See also

- [DEPLOYMENT.md](DEPLOYMENT.md): running a Hub, tokens, HTTPS, and a Hub for
  many users
- [MEMORY_BRANCH_SCOPING.md](MEMORY_BRANCH_SCOPING.md): workspaces and branches
- [design/HUB_SYNC.md](design/HUB_SYNC.md): sync between Hubs, and what it will
  carry

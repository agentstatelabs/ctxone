# Connect to a Hub on another machine

A Hub usually runs on the machine your agents run on. There are two ways to
have your memory on more than one machine:

- **Point every machine at one Hub**: a Hub on a server of your own, one a
  team runs, or a multi-tenant Hub. Every machine then reads and writes the
  same memory, and Lens shows it all in one place. Nothing works while that Hub
  can't be reached.
- **Sync** ([below](#sync-your-own-hub-with-a-remote-one)): each machine keeps
  its own Hub, works offline, and pushes to and pulls from a Hub that serves
  sync. Raw session turns never leave the machine that captured them.

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

## Sync your own Hub with a remote one

Each machine keeps its own Hub and its own store. They push to and pull from
one Hub that serves sync: `ctxone-hub --http --serve-sync`, behind a token and
HTTPS like any shared Hub. On a multi-tenant Hub each token reaches its own
store only.

On each machine, with its own Hub running:

```bash
printf '%s' "$HUB_TOKEN" | ctx remote add hub https://hub.example.com --token-stdin
ctx remote set default hub          # and each project workspace to keep on your other machines
ctx sync                            # pull, then push
ctx sync status
```

- **The token** goes to `~/.ctxone/sync-tokens/hub` (mode 0600), never into the
  store. Or keep it in an environment variable the Hub's own environment sets,
  and name it with `--token-env`.
- **What syncs:** memory, plans and tasks, each session's envelope (title,
  meta, usage, analysis, summary), reminders, taints, plan epochs, and your
  personal namespace (agent rules, `remember --personal`, the project
  registry). Setting the first workspace sets the personal namespace too.
- **What stays on this machine:** raw session turns, on their own local-only
  ref; the history from before you set a workspace (the first push starts
  from where it is then); seals from before workspaces existed; tokens and
  `~/.ctxone`.
- **The first sync of a machine with history of its own** joins the remote's
  history to it in a merge. A task two machines each added offline gets a new
  id on the second to sync, and its blockers follow.
- **Reminders** pulled from another machine don't run their commands here
  until you approve them here: `ctx reminder approve <id>`.
- **Credentials:** every push is scanned. A provider's token format or a
  private key stops a branch's push; `ctx push` prints each finding's path and
  the `ctx forget` that removes it. `ctx push --allow <finding>='<why>'` lets
  one through, and the remote's push log keeps the reason.
- **In the background**, the Hub syncs every five minutes
  (`CTXONE_SYNC_EVERY`, in seconds; `0` turns it off) and within a minute of a
  change. Lens shows each workspace's state on its page.
- **Your own space only, for now.** Every workspace syncs to the same store.
  Sharing a workspace with someone else's waits on AgentStateGraph scoping
  taints and reminders by workspace, so `ctx remote set` refuses a remote in
  another space.

`ctx push` and `ctx pull` run each half on its own, and `--namespace` limits
any of them to one workspace. All of them exit 1 when something didn't go.

## When the Hub can't be reached

Connected directly to a Hub elsewhere, there's no offline mode: `ctx` commands
stop with the Hub unreachable (exit 69), agents' tool calls fail until it's
back, and nothing is queued to send later. Sessions recorded meanwhile can be
sent afterwards with `ctx ingest-session`, since their transcripts are still on
this machine. With sync, each machine's own Hub keeps working, and the next
sync catches up.

## See also

- [DEPLOYMENT.md](DEPLOYMENT.md): running a Hub, tokens, HTTPS, and a Hub for
  many users
- [MEMORY_BRANCH_SCOPING.md](MEMORY_BRANCH_SCOPING.md): workspaces and branches
- [design/HUB_SYNC.md](design/HUB_SYNC.md): how sync between Hubs works, and
  what it carries

# Troubleshooting

Top errors you'll hit and how to fix them. Run `ctx doctor` first — it
catches most of these automatically and suggests fixes.

## 1. `Hub unreachable (http://localhost:3001)`

**Symptom:** any `ctx` command that talks to the Hub fails immediately.
Exit code 69.

**Cause:** the Hub isn't running, isn't on the port you think it is, or is
bound to a different interface.

**Fix:**

```bash
ctx serve --http           # start it
# ... or in another terminal / systemd service / docker
ctx status                 # verify
```

If you have the Hub running on a different port or host:

```bash
export CTX_SERVER=http://my-hub:3001
ctx status
```

Or pass `--server` explicitly.

## 2. `ctx: command not found`

**Cause (macOS / Linux):** `~/.local/bin` isn't on your `PATH`, or the
install script didn't run to completion.

**Fix (macOS / Linux):**

```bash
export PATH="$HOME/.local/bin:$PATH"
# add the above to your ~/.zshrc or ~/.bashrc
```

Verify the binary exists:

```bash
ls -la ~/.local/bin/ctx
```

**Cause (Windows):** PATH changes made by the installer only take effect
in **new** PowerShell windows. Your current shell still has the old PATH.

**Fix (Windows):** close your PowerShell window and open a fresh one.
Or manually add to the current session:

```powershell
$env:Path += ";$env:LOCALAPPDATA\ctxone\bin"
```

Verify the binary exists:

```powershell
Get-Item "$env:LOCALAPPDATA\ctxone\bin\ctx.exe"
```

If missing, re-run the installer:

```bash
curl -sSL https://raw.githubusercontent.com/ctxone/ctxone/main/install.sh | sh
```

## 3. `No memories found for '<topic>'` but you know the fact is there

**Cause A — wrong branch.** You're reading from `main` but the fact was
written to another branch.

```bash
ctx branches             # see which branch has your fact
ctx --branch <name> recall "<topic>"
```

**Cause B — search vs recall mismatch.** `ctx recall` tokenizes the query
and drops stopwords. "the status" becomes `["status"]` since "the" is a
stopword. Try a more specific single word or use `ctx search` which does
literal substring matching.

**Cause C — the fact is on a pinned path.** Pinned memories are split into
`/title` and `/body` fields. `recall` dedups these, but `search` doesn't —
check both:

```bash
ctx search "<term>"            # literal
ctx ls /memory/pinned          # see pinned paths
```

**Cause D — the fact was forgotten.** Check the commit history:

```bash
ctx log -n 100 | grep -i "<term>"
```

If it was deleted, you can still see the old commit in `blame` but the
current state won't have it.

## 4. `ctx init` wrote a config but Claude Code / Cursor still doesn't see CtxOne

**Cause A — the AI tool needs a restart.** Most MCP clients load config on
startup. Restart Claude Code / Cursor / VS Code after running `ctx init`.

**Cause B — wrong config scope.** `ctx init` writes project-level configs by
default (`.mcp.json` in cwd). If you want global, add `--global`.

**Cause C — path mismatch.** Check where `ctx init` actually wrote:

```bash
ctx init --dry-run
```

Copy the path. Open it in an editor. Verify the `mcpServers.ctxone` entry
points at your actual `ctxone-hub` binary.

**Cause D — the Hub binary moved.** If you reinstalled, the old `.mcp.json`
may point at a stale path. Re-run `ctx init` to refresh.

## 5. Ratio stuck near 1.0x — no savings

**Cause A — graph is tiny.** With 5 facts total and recalling 4 of them,
flat ≈ sent. Savings don't kick in until the graph is bigger than what any
given recall returns.

**Cause B — overly broad recall.** `ctx recall "project"` on a
project-heavy graph matches every fact. Try a more specific query.

**Cause C — too much pinned content.** If you've pinned several long docs,
pinned content alone eats the budget. Review with `ctx pinned` and unpin
(via `ctx forget`) anything that's not actually critical.

See [TOKEN_SAVINGS.md](TOKEN_SAVINGS.md) for the full breakdown.

## 6. `branch not found: <name>` when writing

**Cause:** you tried to write to a branch that doesn't exist yet. Branches
must be created explicitly.

**Fix:**

```bash
ctx branch <name>                 # create it
ctx --branch <name> remember "..." # now writes work
```

Or create it from a specific ref:

```bash
ctx branch <name> --from main
```

## 7. `ctx prime` reports "No sections found in <file>"

**Cause:** the markdown file has no H1 or H2 headings. `ctx prime` only
splits at `# ` and `## ` (not `###` or deeper).

**Fix:** either add headings, or accept that the whole file becomes one
"Intro" section.

If you want deeper headings to count, open an issue — we can extend the
parser.

## 8. Cargo build failing with `thiserror not found in workspace.dependencies`

**Cause:** you ran `cargo build` from the root without initializing the
`engine/` git submodule.

**Fix:**

```bash
git submodule update --init --recursive
cargo build --workspace
```

Or clone recursively from the start:

```bash
git clone --recursive https://github.com/ctxone/ctxone.git
```

## 9. `ctx tail` shows nothing when I'm writing in another terminal

**Cause A — wrong branch.** `ctx tail` reads the branch you specified (or
`main` by default). If the writes are going to a different branch, tail won't
see them.

```bash
ctx tail --branch <other_branch>
```

**Cause B — polling interval.** Default is 2000ms. Writes within that window
show up on the next poll. Lower with `--interval 500`.

**Cause C — the write failed.** Check the exit code of the `ctx remember`
command in the other terminal.

## 10. Postgres Hub errors: `database "ctxone" does not exist`

**Cause:** the Postgres database itself isn't created. CtxOne creates the
*schema* on init but expects the database to already exist.

**Fix:**

```sql
-- Connect to postgres as a superuser
CREATE DATABASE ctxone;
CREATE USER ctxone WITH PASSWORD 'secret';
GRANT ALL PRIVILEGES ON DATABASE ctxone TO ctxone;
```

Then:

```bash
export DATABASE_URL=postgres://ctxone:secret@localhost:5432/ctxone
ctx serve --http --storage postgres
```

The Hub will create its tables on first run.

---

## Enabling verbose logs

The Hub uses the `tracing` crate. Set `RUST_LOG` before starting it to
control verbosity:

```bash
# Default — info-level startup and recall telemetry
ctx serve --http

# Debug — also see prime/forget/remember request details
RUST_LOG=debug ctx serve --http

# Trace — every field of every span, useful for deep debugging
RUST_LOG=trace ctx serve --http

# Scoped — only enable debug for CtxOne's own code
RUST_LOG=ctxone_hub=debug ctx serve --http

# Combined — debug CtxOne, info HTTP request traces from tower-http
RUST_LOG=ctxone_hub=debug,tower_http=info ctx serve --http
```

All logs go to **stderr**, so they never corrupt the stdio MCP channel
when the Hub runs as an MCP server.

In HTTP mode, every `recall` call emits an `info`-level line with the
topic, tokens sent, and savings ratio — useful for watching memory earn
its keep in real time. Writes log at `debug` level.

## Still stuck?

- Run `ctx doctor` — it catches most infrastructure problems automatically.
- Check the Hub logs. If running via `ctx serve`, errors print to stderr in
  that terminal. Use `RUST_LOG=debug` for more detail.
- Open an issue at https://github.com/ctxone/ctxone/issues with: what you
  tried, what you expected, what you got, and the output of `ctx --version`
  and `ctx doctor`.

# Data Safety

Your memory database is the only thing in CTXone that can't be regenerated.
Lose it and you lose every fact, plan, and provenance record the hub ever
wrote down. So we treat it as the crown jewel.

This page covers the four layers of defense and the two recovery commands.
Every defense is on by default — you don't have to opt in.

## What's protected

The defenses cover the SQLite backend (`./target/ctxone.db` by default, or
`~/.ctxone/memory.db` for the system-wide hub). The `memory://` and
`postgres://` backends rely on their own durability story and skip these
layers.

## Layer 1 — `--init` is required to create a new db

Running `ctxone-hub --path ./some/path.db` against a missing file used to
silently create it. That's how the 2026-04-28 incident started: a stray
`ctxone-hub --version` invocation against the wrong cwd birthed a stub
`./ctxone.db`, which then got `rm`'d, which kicked off a much worse chain.

Now the hub refuses to create a missing db unless you pass `--init`:

```bash
# First time — opt in:
ctxone-hub --path ./my-new.db --init

# Subsequent runs — same command without --init is fine:
ctxone-hub --path ./my-new.db
```

Stray invocations no longer leave debris.

## Layer 2 — strict argv, no storage on diagnostics

`--version` and `--help` short-circuit before any storage code runs. Unknown
flags exit `64` (`EX_USAGE`) instead of being silently ignored. You can
type `ctxone-hub --version` from anywhere and trust that nothing on disk
moved.

## Layer 3 — automatic snapshots via `VACUUM INTO`

Two snapshot triggers, both on by default:

- **Startup snapshot** — every time the hub opens a sqlite db, it copies
  the file to `<db>.bak.<utc-iso>` using SQLite's `VACUUM INTO`. This is
  a consistent online backup — no need to stop writes.
- **Rolling snapshot** — every 30 minutes (configurable via
  `CTXONE_BACKUP_INTERVAL_SECS`, set to `0` to disable), the hub repeats
  the snapshot.

The hub keeps the last `K` snapshots (default `5`, configurable via
`CTXONE_BACKUP_KEEP`) and prunes older ones. Snapshots live next to the
db file, so any backup tool that grabs the db's directory grabs them too.

Example layout after a few hours:

```
target/
├── ctxone.db
├── ctxone.db.bak.20260428T143022Z
├── ctxone.db.bak.20260428T150022Z
├── ctxone.db.bak.20260428T153022Z
├── ctxone.db.bak.20260428T160022Z
└── ctxone.db.bak.20260428T163022Z   ← newest
```

## Layer 4 — PID lockfile + inode-drift watchdog

On startup the hub writes `<db>.lock` containing
`{"pid": N, "started_at_unix": N, "hub_version": "..."}`. If a second hub
tries to open the same db and the lockfile's PID is alive, it refuses
with a clear error:

```
database is already locked by ctxone-hub pid 12345 (lockfile: ./target/ctxone.db.lock);
refusing to start a second hub against the same db
```

Stale locks (PID gone) are reclaimed automatically with a warning.

A background watchdog stats the db file every 30 seconds and compares
`(dev, inode)` against what was open at startup. If the file is replaced
or unlinked under a running hub — e.g. `rm` or `mv` from another shell —
the watchdog logs a single WARN within 30 seconds:

```
database file replaced — current process is still writing to the OLD inode.
Restart the hub to attach to the new file.
```

Operator gets ~30 seconds of warning instead of silent loss until the
next restart.

## Recovery: `ctx db backup` and `ctx db restore`

Two CLI subcommands wrap the snapshot/restore loop.

### `ctx db backup`

Triggers an immediate snapshot via the hub's admin endpoint and prints
the path:

```bash
$ ctx db backup
Snapshot: /Users/user/.ctxone/memory.db.bak.20260428T172145Z
```

Pass `--out PATH` to write to a specific destination. Use this before
risky operations (schema migrations, bulk imports, big refactors).

### `ctx db restore`

Swaps a snapshot back into place. The hub must be stopped first (the
command refuses to proceed if the lockfile shows a live PID — same gate
that prevents two hubs racing on one db).

```bash
$ ctx db restore ./target/ctxone.db.bak.20260428T143022Z
About to restore from /Users/user/.../bak.20260428T143022Z
Current db will be moved to /Users/user/.../ctxone.db.pre-restore-1745882519
Continue? [y/N]:
```

Pass `--yes` to skip the confirmation. The current db is preserved at
`<db>.pre-restore-<unix-ts>` so you can roll the restore back if the
snapshot turns out to be wrong.

## `ctx doctor` checks

`ctx doctor` runs three db-safety checks alongside its config audits:

- **db inode drift** — for every known db lockfile with a live PID, the
  db file must still exist. Fires if a hub is writing to an unlinked
  inode (the 2026-04-28 failure mode).
- **stray db files** — counts ctxone.db across `cwd`, `target/`,
  `~/.ctxone`, and the canonical path. Warns if more than one exists —
  somebody has run the hub from the wrong cwd and birthed a stub.
- **recent backups** — at least one `<db>.bak.*` sibling must be modified
  within the last 24 hours. If not, suggests `ctx db backup`.

Each failed check ships with a one-line fix in the `Suggestions:` block.

## What to do when something goes wrong

1. **Hub won't start: "database is already locked"** — there's another
   hub running against the same db, or a stale lockfile from a hub that
   was killed mid-write. `ps -ef | grep ctxone-hub` to confirm; if no
   live process, delete the `<db>.lock` file and try again. Stale locks
   normally clean themselves but a `kill -9` can leave one behind.

2. **`ctx doctor` flags inode drift** — the hub is writing to an inode
   that no longer has a name on disk. Restart the hub immediately
   (writes will hit the new inode, but writes since the deletion are
   trapped on the old one). Then `ctx db restore` from the most recent
   pre-deletion snapshot.

3. **Db file is corrupt or missing entirely** — pick the newest viable
   `<db>.bak.<utc>`, stop the hub, run `ctx db restore <snapshot>`. The
   current (broken) db is preserved at `<db>.pre-restore-<ts>` for
   forensics.

4. **You want to roll back a few minutes** — every snapshot is a full
   db. Pick the one just before the bad write, restore it, restart the
   hub. You'll lose anything written since the snapshot.

## Configuration summary

| Env var | Default | Effect |
|---|---|---|
| `CTXONE_BACKUP_INTERVAL_SECS` | `1800` (30min) | Rolling snapshot cadence. `0` disables rolling snapshots (startup snapshot still runs). |
| `CTXONE_BACKUP_KEEP` | `5` | How many snapshots to keep before pruning oldest. |

That's it. Everything else is automatic.

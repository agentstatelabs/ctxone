# Example 04 — Bulk import facts

Import facts from a plain-text file (one fact per line) or a JSONL file
(one JSON object per line with fact + metadata).

## What it does

Two scripts:

- **`import-txt.sh <file>`** — reads one fact per line, imports each with
  default importance and a shared context.
- **`import-jsonl.sh <file>`** — reads structured JSONL with per-fact
  importance, context, and tags.

Both report the number of facts stored at the end.

## Sample data

See `sample.txt` and `sample.jsonl` for examples. Both contain 5 realistic
facts.

## Run it

```bash
# Plain text (uses --context imported, importance medium)
./import-txt.sh sample.txt

# JSONL (respects per-fact metadata)
./import-jsonl.sh sample.jsonl
```

Verify:

```bash
ctx search "BSL"
ctx ls /memory/licensing
ctx ls /memory/imported
```

## Sample output

```
$ ./import-txt.sh sample.txt
Importing 5 facts from sample.txt...
..... done
Imported 5 facts under /memory/imported/

$ ./import-jsonl.sh sample.jsonl
Importing 5 facts from sample.jsonl...
..... done
Imported 5 facts with per-fact metadata
```

## When to use each

- **`import-txt.sh`** — quick-and-dirty: a list of facts from a meeting,
  a brain dump, or a tsv export. All facts get the same context.
- **`import-jsonl.sh`** — structured import: each fact gets its own
  importance, context, and tags. Use when exporting from another system.

## Extending

Both scripts are short enough to modify. Common tweaks:

- Add `--branch` support: `ctx --branch $BRANCH remember "..."`
- Output JSON for each import: use `--format json` and log to a file
- Parallelize: pipe through `xargs -P 8` (the Hub handles concurrency)

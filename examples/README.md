# Examples

Runnable example projects showing CtxOne patterns end-to-end. Each example
is a self-contained directory with its own README and setup instructions.

For recipe snippets (copy-paste), see [../docs/COOKBOOK.md](../docs/COOKBOOK.md).

## The examples

| Directory | What it shows |
|-----------|---------------|
| [01-git-pre-push](01-git-pre-push/) | Git pre-push hook that primes your README on every push |
| [02-daily-digest](02-daily-digest/) | Cron job that emits a daily digest of memory activity |
| [03-experiment-branch](03-experiment-branch/) | Try a memory change on a branch, diff vs main, then decide |
| [04-bulk-import](04-bulk-import/) | Import N facts from a plain-text or JSONL file |

## Prerequisites

All examples assume you have CtxOne installed and a Hub running:

```bash
curl -sSL https://raw.githubusercontent.com/ctxone/ctxone/main/install.sh | sh
ctx serve --http &
ctx doctor  # verify
```

See [../docs/QUICKSTART.md](../docs/QUICKSTART.md) if any of that is unclear.

## Contributing an example

Open a PR with a new directory. Minimum:

- `README.md` — what it does, how to run it, expected output
- The actual script(s) — self-contained, with comments
- No external dependencies beyond `ctx`, `jq`, and standard Unix tools

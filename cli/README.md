# ctxone

The `ctx` command-line tool for [CTXone](https://github.com/agentstatelabs/ctxone), the system of record for AI
agents. CTXone stores decisions, plans and full work sessions in a shared Hub
so the next agent, or the next teammate, recalls them instead of re-deriving
them.

```sh
cargo install ctxone      # installs the `ctx` binary
```

`ctx` talks to a running Hub
([`ctxone-hub`](https://crates.io/crates/ctxone-hub)). It handles memory
(`remember`, `recall`), plans and tasks, branches, taint, session import from
Claude Code, Codex, Gemini and Cursor, and the always-on Hub service
(`ctx service install`).

The crate is named `ctxone` because `ctx` is taken on crates.io. The Python
client is also called `ctxone`, on PyPI (`pip install ctxone`).

## License

[Business Source License 1.1](https://github.com/agentstatelabs/ctxone/blob/main/LICENSE) (SPDX: `BUSL-1.1`). Each
version converts to the Apache License 2.0 on its Change Date; see the license
for the Additional Use Grant and the exact terms.

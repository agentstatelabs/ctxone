# ctxone-hub

The [CTXone](https://github.com/agentstatelabs/ctxone) Hub: one daemon that serves the MCP server, the HTTP API and
the Lens web UI, which is built into the binary. It is the memory interface
for AI tools, built on [AgentStateGraph](https://crates.io/crates/agentstategraph),
so everything it records is content-addressed, branchable and blameable.

```sh
cargo install ctxone-hub
ctxone-hub --http --lens
```

The [`ctx`](https://crates.io/crates/ctxone) CLI talks to the Hub. It can also
install the Hub as a login or boot service (`ctx service install`).

## License

[Business Source License 1.1](https://github.com/agentstatelabs/ctxone/blob/main/LICENSE) (SPDX: `BUSL-1.1`). Each
version converts to the Apache License 2.0 on its Change Date; see the license
for the Additional Use Grant and the exact terms.

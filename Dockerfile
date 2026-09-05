# CtxOne — multi-stage build
# Produces a minimal debian-slim image containing ctx and ctxone-hub.

# -- Stage 1: Build the Lens frontend --
#
# server/src/lens.rs embeds web/build/ at compile time:
#     #[derive(Embed)] #[folder = "../web/build/"]
# That folder is gitignored and produced by `vite build`, so cargo CANNOT
# compile ctxone-hub until this stage has run. .gitlab-ci.yml gets this right
# — its `frontend` stage runs before `check` and passes web/build/ down as an
# artifact — but the Dockerfile never replicated it, so every container build
# failed with "folder '/build/server/../web/build/' does not exist".
#
# Node 22 to match CI: Vite 8 / Svelte 5 require Node >= 20.19.
FROM node:22-alpine AS lens
WORKDIR /web

# Manifests first so npm ci is cached independently of source edits.
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund

COPY web/ ./
RUN npm run build

# -- Stage 2: Build Rust binaries --
FROM rust:1.88-slim AS builder

WORKDIR /build

# Install build dependencies
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace root + Cargo.lock for reproducible builds
COPY Cargo.toml Cargo.lock ./

# Copy our crates. There is no engine/ directory: the ASG engine used to be a
# git submodule, but Cargo.toml now pins those crates as git dependencies
# (see its "was a GitHub submodule" note), so cargo fetches them during the
# build. Commit 3850ce9 removed the directory, left the old
# `COPY engine/ engine/` here, and deleted the docker workflow that would have
# caught it — so this file quietly stopped building. It DID build before that.
COPY cli/ cli/
COPY server/ server/

# The cli crate embeds docs/AGENTS.md via include_str!("../../docs/AGENTS.md"),
# so the docs file must be present at build time even though it isn't inside
# any crate. Copy just that file to keep the build context tight.
COPY docs/AGENTS.md docs/AGENTS.md

# The embedded Lens assets, from the node stage above. Must land before the
# cargo build: rust-embed reads this folder at compile time, not at runtime.
COPY --from=lens /web/build/ web/build/

# Build just the two binaries we ship, not the whole workspace
RUN cargo build --release -p ctx -p ctxone-hub \
    && strip target/release/ctx target/release/ctxone-hub

# -- Stage 3: Runtime --
FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/ctx /usr/local/bin/ctx
COPY --from=builder /build/target/release/ctxone-hub /usr/local/bin/ctxone-hub

# Data volume for the SQLite database
VOLUME /data

EXPOSE 3001

HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD curl -fsS http://localhost:3001/api/health || exit 1

# Default: run the Hub in HTTP mode, pointing at /data/ctxone.db.
#
# --init is required here. The Hub deliberately refuses to create a missing db
# (server/src/main.rs exits 66) so that a typo'd --path cannot silently hand an
# operator an empty graph. In a container there is no typo to guard against —
# the path is fixed in this CMD, not typed — while a fresh `-v ctxone-data:/data`
# volume is empty by definition. Without --init the documented
#   docker run -p 3001:3001 -v ctxone-data:/data <image>
# exits 66 on first run. --init is a no-op once the file exists, so restarts and
# existing volumes are unaffected.
CMD ["ctxone-hub", "--http", "--port", "3001", "--path", "/data/ctxone.db", "--init"]

# CtxOne — multi-stage build
# Produces a minimal debian-slim image containing ctx and ctxone-hub.

# -- Stage 1: Build Rust binaries --
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
# build. The stale `COPY engine/ engine/` left behind by that migration is why
# this image never built and why every documented `docker run` failed.
COPY cli/ cli/
COPY server/ server/

# The cli crate embeds docs/AGENTS.md via include_str!("../../docs/AGENTS.md"),
# so the docs file must be present at build time even though it isn't inside
# any crate. Copy just that file to keep the build context tight.
COPY docs/AGENTS.md docs/AGENTS.md

# Build just the two binaries we ship, not the whole workspace
RUN cargo build --release -p ctx -p ctxone-hub \
    && strip target/release/ctx target/release/ctxone-hub

# -- Stage 2: Runtime --
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

# Default: run the Hub in HTTP mode, pointing at /data/ctxone.db
CMD ["ctxone-hub", "--http", "--port", "3001", "--path", "/data/ctxone.db"]

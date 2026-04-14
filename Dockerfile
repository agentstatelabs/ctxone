# CtxOne — multi-stage build
# Stage 1: Build Rust binaries (ctx + ctxone-hub)
FROM rust:1.86-slim AS builder-rust

WORKDIR /build
COPY Cargo.toml ./
COPY cli/ cli/
COPY server/ server/

# Copy engine submodule
COPY engine/ engine/

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
RUN cargo build --workspace --release && strip target/release/ctx target/release/ctxone-hub

# Stage 2: Build SvelteKit web app
FROM node:22-slim AS builder-web

WORKDIR /build/web
COPY web/package.json web/package-lock.json* ./
RUN npm ci
COPY web/ .
RUN npm run build

# Stage 3: Runtime
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y ca-certificates curl && rm -rf /var/lib/apt/lists/*

# Rust binaries
COPY --from=builder-rust /build/target/release/ctx /usr/local/bin/ctx
COPY --from=builder-rust /build/target/release/ctxone-hub /usr/local/bin/ctxone-hub

# Web app
COPY --from=builder-web /build/web/build /app/web/build
COPY --from=builder-web /build/web/package.json /app/web/package.json

WORKDIR /app
EXPOSE 3001 3000

CMD ["ctxone-hub", "--http", "--port", "3001", "--path", "/data/ctxone.db"]

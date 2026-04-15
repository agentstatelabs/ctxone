# CtxOne Lens — production image
#
# Multi-stage Node build:
#   1. deps        — install npm dependencies with cache-friendly layering
#   2. builder     — run SvelteKit build via @sveltejs/adapter-node
#   3. runtime     — Alpine runtime with just the built server + production deps
#
# The runtime listens on port 3000 by default (SvelteKit adapter-node
# convention). Override with PORT. Talks to the Hub via PUBLIC_CTXONE_API_URL,
# which the SvelteKit app reads at build time. Set that in docker-compose.

# -- 1. deps ----------------------------------------------------------------
FROM node:22-alpine AS deps
WORKDIR /app

# Copy only the manifests first so this layer is cached whenever
# package.json / package-lock.json don't change.
COPY package.json package-lock.json ./
RUN npm ci --no-audit --no-fund

# -- 2. builder -------------------------------------------------------------
FROM node:22-alpine AS builder
WORKDIR /app

COPY --from=deps /app/node_modules ./node_modules
COPY . .

# VITE_CTXONE_API_URL is baked into the client bundle at build time.
# The compose file passes it as an ARG so both build-time and run-time
# settings line up.
ARG VITE_CTXONE_API_URL=http://localhost:3001
ENV VITE_CTXONE_API_URL=${VITE_CTXONE_API_URL}

RUN npm run build \
    && npm prune --omit=dev

# -- 3. runtime -------------------------------------------------------------
FROM node:22-alpine AS runtime
WORKDIR /app

# Non-root for defense in depth.
RUN addgroup -S lens && adduser -S lens -G lens

# SvelteKit adapter-node layout: build/ is the server bundle, and we
# keep the pruned node_modules plus package.json for module resolution.
COPY --from=builder --chown=lens:lens /app/build ./build
COPY --from=builder --chown=lens:lens /app/node_modules ./node_modules
COPY --from=builder --chown=lens:lens /app/package.json ./package.json

USER lens

ENV NODE_ENV=production
ENV HOST=0.0.0.0
ENV PORT=3000

EXPOSE 3000

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD wget -q -O- http://localhost:3000 > /dev/null || exit 1

CMD ["node", "build"]

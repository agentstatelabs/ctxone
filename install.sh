#!/bin/sh
# CtxOne installer — downloads `ctx` + `ctxone-hub` to ~/.local/bin.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/agentstatelabs/ctxone/main/install.sh | sh
#
# Env overrides:
#   INSTALL_DIR             where to install (default: ~/.local/bin)
#   CTXONE_VERSION          pin a release tag (default: latest)
#   CTXONE_RELEASES_REPO    release asset repo (default: agentstatelabs/ctxone-releases)
set -e

# ─── Configuration ──────────────────────────────────────────────────────────
RELEASES_REPO="${CTXONE_RELEASES_REPO:-agentstatelabs/ctxone-releases}"
SOURCE_REPO="agentstatelabs/ctxone"
INSTALL_DIR="${INSTALL_DIR:-${HOME}/.local/bin}"
BINS="ctx ctxone-hub"

# ─── Pretty output ──────────────────────────────────────────────────────────
BOLD=''; DIM=''; GREEN=''; YELLOW=''; CYAN=''; RESET=''
if [ -t 1 ]; then
    BOLD=$(printf '\033[1m'); DIM=$(printf '\033[2m')
    GREEN=$(printf '\033[32m'); YELLOW=$(printf '\033[33m')
    CYAN=$(printf '\033[36m'); RESET=$(printf '\033[0m')
fi
say()  { printf "%s\n" "$1"; }
ok()   { printf "  ${GREEN}\xe2\x9c\x93${RESET} %s\n" "$1"; }
info() { printf "  ${DIM}\xe2\x80\x93${RESET} %s\n" "$1"; }
warn() { printf "  ${YELLOW}!${RESET} %s\n" "$1"; }
die()  { printf "  ${YELLOW}\xe2\x9c\x97${RESET} %s\n" "$1" >&2; exit 1; }

# ─── Platform detection ─────────────────────────────────────────────────────
OS_RAW="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH_RAW="$(uname -m)"

case "$ARCH_RAW" in
    x86_64|amd64)   ARCH="x86_64" ;;
    aarch64|arm64)  ARCH="aarch64" ;;
    *) die "Unsupported architecture: $ARCH_RAW" ;;
esac

case "$OS_RAW" in
    linux)  TARGET="${ARCH}-unknown-linux-gnu" ;;
    darwin) TARGET="${ARCH}-apple-darwin" ;;
    *)      die "Unsupported OS: $OS_RAW (try install.ps1 on Windows)" ;;
esac

say ""
say "${BOLD}${CYAN}CtxOne installer${RESET}"
say "  Target: ${TARGET}"
say "  Dir:    ${INSTALL_DIR}"
say ""

mkdir -p "$INSTALL_DIR"

# ─── Resolve release tag ────────────────────────────────────────────────────
if [ -n "${CTXONE_VERSION:-}" ]; then
    TAG="$CTXONE_VERSION"
    info "Using pinned version: ${TAG}"
else
    info "Resolving latest release from ${RELEASES_REPO}..."
    TAG=$(curl -fsSL "https://api.github.com/repos/${RELEASES_REPO}/releases/latest" 2>/dev/null \
        | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' \
        | head -1)
    if [ -z "$TAG" ]; then
        warn "Could not resolve latest release from ${RELEASES_REPO}."
        say ""
        say "Build from source instead:"
        say ""
        say "  ${DIM}git clone https://github.com/${SOURCE_REPO}.git${RESET}"
        say "  ${DIM}cd ctxone${RESET}"
        say "  ${DIM}cargo build --workspace --release${RESET}"
        say "  ${DIM}cp target/release/ctx target/release/ctxone-hub ${INSTALL_DIR}/${RESET}"
        say ""
        die "Aborting install."
    fi
    info "Latest is ${TAG}"
fi

# ─── Download + extract the tarball ────────────────────────────────────────
# Release layout: ctxone-<TAG>-<TARGET>/{ctx,ctxone-hub}
TARBALL="ctxone-${TAG}-${TARGET}.tar.gz"
URL="https://github.com/${RELEASES_REPO}/releases/download/${TAG}/${TARBALL}"
TMP=$(mktemp -d -t ctxone-install.XXXXXX)
trap 'rm -rf "$TMP"' EXIT

say ""
say "${BOLD}Downloading ${TAG} (${TARGET})...${RESET}"
info "${URL}"
if ! curl -fsSL "$URL" -o "${TMP}/${TARBALL}"; then
    die "Download failed. Check that ${TARGET} is included in this release."
fi

info "Extracting..."
if ! tar -xzf "${TMP}/${TARBALL}" -C "$TMP" --strip-components=1; then
    die "Extraction failed."
fi

for BIN in $BINS; do
    if [ ! -f "${TMP}/${BIN}" ]; then
        die "Tarball is missing ${BIN} — release artifact is malformed."
    fi
    install -m 0755 "${TMP}/${BIN}" "${INSTALL_DIR}/${BIN}"
    ok "${BIN}"
done

say ""
ok "Installed to ${INSTALL_DIR}"

# ─── PATH check ─────────────────────────────────────────────────────────────
case ":$PATH:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        say ""
        warn "${INSTALL_DIR} is not on your PATH. Add this to your shell rc:"
        say "    export PATH=\"${INSTALL_DIR}:\$PATH\""
        ;;
esac

say ""
say "${BOLD}Get started:${RESET}"
say "  ${DIM}ctx init${RESET}     # Configure your AI tools"
say "  ${DIM}ctx status${RESET}   # Check Hub connection"
say "  ${DIM}ctx serve${RESET}    # Start the Hub server"

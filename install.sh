#!/bin/sh
# CtxOne installer — downloads ctx CLI and ctxone-hub to ~/.local/bin
set -e

REPO="ctxone/ctxone"
INSTALL_DIR="${HOME}/.local/bin"

# Detect OS and architecture, then map to a Rust target triple
OS_RAW="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH_RAW="$(uname -m)"

case "$ARCH_RAW" in
    x86_64|amd64) ARCH="x86_64" ;;
    aarch64|arm64) ARCH="aarch64" ;;
    *) echo "Unsupported architecture: $ARCH_RAW"; exit 1 ;;
esac

case "$OS_RAW" in
    linux)  TARGET="${ARCH}-unknown-linux-gnu" ;;
    darwin) TARGET="${ARCH}-apple-darwin" ;;
    *)      echo "Unsupported OS: $OS_RAW"; exit 1 ;;
esac

OS="$OS_RAW"

echo "CtxOne installer"
echo "  Target: $TARGET"
echo "  Dir:    $INSTALL_DIR"
echo ""

# Create install directory
mkdir -p "$INSTALL_DIR"

# Get latest release tag
LATEST=$(curl -sL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')

if [ -z "$LATEST" ]; then
    echo "No releases found yet. Build from source:"
    echo ""
    echo "  git clone --recursive https://github.com/${REPO}.git"
    echo "  cd ctxone"
    echo "  cargo build --workspace --release"
    echo "  cp target/release/ctx target/release/ctxone-hub ~/.local/bin/"
    exit 1
fi

echo "Installing CtxOne $LATEST..."

# Download binaries
for BIN in ctx ctxone-hub; do
    URL="https://github.com/${REPO}/releases/download/${LATEST}/${BIN}-${TARGET}"
    echo "  Downloading $BIN..."
    if ! curl -fsSL "$URL" -o "${INSTALL_DIR}/${BIN}"; then
        echo "  Failed: $URL"
        exit 1
    fi
    chmod +x "${INSTALL_DIR}/${BIN}"
done

echo ""
echo "Installed to $INSTALL_DIR"

# Check PATH
case ":$PATH:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        echo ""
        echo "Add to your PATH:"
        echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
        ;;
esac

echo ""
echo "Get started:"
echo "  ctx init        # Configure your AI tools"
echo "  ctx status      # Check Hub connection"
echo "  ctx serve       # Start the Hub server"

#!/bin/sh
# CtxOne uninstaller
# Removes binaries, app data, and the ctxone MCP entry from all AI tool configs.
set -e

BOLD='\033[1m'
DIM='\033[2m'
RED='\033[0;31m'
GREEN='\033[0;32m'
RESET='\033[0m'

ok()  { printf "  ${GREEN}✓${RESET} %s\n" "$1"; }
skip(){ printf "  ${DIM}–${RESET} %s\n" "$1"; }
warn(){ printf "  ${RED}!${RESET} %s\n" "$1"; }

# ── helpers ──────────────────────────────────────────────────────────────────

# Remove the "ctxone" key from mcpServers in a JSON file, in-place.
# Requires python3 (available by default on macOS 12+).
remove_from_json() {
    FILE="$1"
    [ -f "$FILE" ] || { skip "not found: $FILE"; return; }

    python3 - "$FILE" <<'PYEOF'
import json, sys
path = sys.argv[1]
try:
    with open(path) as f:
        data = json.load(f)
except Exception as e:
    print(f"  ! could not parse {path}: {e}")
    sys.exit(0)

changed = False
for key in ("mcpServers", "servers"):
    if key in data and "ctxone" in data[key]:
        del data[key]["ctxone"]
        changed = True

if changed:
    with open(path, "w") as f:
        json.dump(data, f, indent=2)
        f.write("\n")
    print(f"  ✓ removed ctxone from {path}")
else:
    print(f"  – ctxone not present in {path}")
PYEOF
}

# Remove the [mcp_servers.ctxone] entry from a TOML file, in-place.
remove_from_toml() {
    FILE="$1"
    [ -f "$FILE" ] || { skip "not found: $FILE"; return; }

    python3 - "$FILE" <<'PYEOF'
import sys, re
path = sys.argv[1]
try:
    with open(path) as f:
        text = f.read()
except Exception as e:
    print(f"  ! could not read {path}: {e}")
    sys.exit(0)

# Remove [mcp_servers.ctxone] block (everything up to the next [section] or EOF)
pattern = r'\[mcp_servers\.ctxone\][^\[]*'
new_text, n = re.subn(pattern, '', text, flags=re.DOTALL)
if n:
    with open(path, "w") as f:
        f.write(new_text)
    print(f"  ✓ removed ctxone from {path}")
else:
    print(f"  – ctxone not present in {path}")
PYEOF
}

# ── main ─────────────────────────────────────────────────────────────────────

printf "\n${BOLD}CtxOne Uninstaller${RESET}\n\n"

# 1. Binaries
printf "${BOLD}Binaries${RESET}\n"
if command -v brew >/dev/null 2>&1 && brew list ctxone >/dev/null 2>&1; then
    brew uninstall ctxone && ok "brew uninstall ctxone"
else
    for BIN in ctx ctxone-hub; do
        for DIR in "$HOME/.local/bin" /usr/local/bin /opt/homebrew/bin; do
            TARGET="$DIR/$BIN"
            if [ -f "$TARGET" ]; then
                rm -f "$TARGET" && ok "removed $TARGET"
            fi
        done
    done
fi

# 2. App data
printf "\n${BOLD}App data${RESET}\n"
DATA_DIR="$HOME/Library/Application Support/ctxone"
if [ -d "$DATA_DIR" ]; then
    printf "  Remove %s? [y/N] " "$DATA_DIR"
    read -r REPLY
    case "$REPLY" in
        [yY]*) rm -rf "$DATA_DIR" && ok "removed $DATA_DIR" ;;
        *)     skip "kept $DATA_DIR" ;;
    esac
else
    skip "no app data found"
fi

# 3. MCP integrations
printf "\n${BOLD}MCP integrations${RESET}\n"

# Claude Code — project .mcp.json and global ~/.claude/settings.json
remove_from_json ".mcp.json"
remove_from_json "$HOME/.claude/settings.json"

# Claude Desktop
remove_from_json "$HOME/Library/Application Support/Claude/claude_desktop_config.json"

# Cursor
remove_from_json "$HOME/.cursor/mcp.json"
remove_from_json ".cursor/mcp.json"

# VS Code
remove_from_json "$HOME/Library/Application Support/Code/User/settings.json"
remove_from_json ".vscode/mcp.json"

# Codex
remove_from_toml "$HOME/.codex/config.toml"

# Gemini CLI
remove_from_json "$HOME/.gemini/settings.json"
remove_from_json ".gemini/settings.json"

# Grok CLI
remove_from_json "$HOME/.grok/settings.json"
remove_from_json ".grok/settings.json"

printf "\n${BOLD}Done.${RESET} CtxOne has been uninstalled.\n\n"

# Running CtxOne on Windows

A complete Windows guide: install, first run, AI tool setup, background
service, update, uninstall, and troubleshooting. For the cross-platform
quickstart, see [QUICKSTART.md](QUICKSTART.md).

## TL;DR

```powershell
# Install (one line)
iwr https://raw.githubusercontent.com/ctxone/ctxone/main/install.ps1 | iex

# Open a new PowerShell window, then
ctx serve --http            # in one window
ctx demo                    # in another
ctx init                    # wire into Claude Code, Cursor, VS Code...
```

Three files end up in `%LOCALAPPDATA%\ctxone\bin\`: `ctx.exe`,
`ctxone-hub.exe`, and the memory database at `%APPDATA%\ctxone\memory.db`.

## Which install path is right for you?

| You want... | Use |
|-------------|-----|
| The lightest, most native experience | **`install.ps1`** (recommended) |
| To run CtxOne without touching your Windows PATH | **Docker Desktop** |
| To hack on the source | **Build from source** |

All three options produce a working Hub. They don't conflict — you can
install via `install.ps1` and later `cargo build` from source without
breaking anything.

### Option 1: install.ps1 (recommended)

Open **PowerShell** (not Command Prompt) and run:

```powershell
iwr https://raw.githubusercontent.com/ctxone/ctxone/main/install.ps1 | iex
```

This downloads the latest release's `ctx.exe` and `ctxone-hub.exe` into
`%LOCALAPPDATA%\ctxone\bin` and adds that directory to your user PATH.

**Important:** PATH changes only take effect in **new** PowerShell
windows. Close and reopen your shell before running `ctx --version`.

If the installer is blocked by PowerShell's execution policy, run:

```powershell
Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass
iwr https://raw.githubusercontent.com/ctxone/ctxone/main/install.ps1 | iex
```

This only affects the current window — it doesn't weaken your system
policy.

### Option 2: Docker Desktop

If you already run Docker Desktop on Windows, you can skip `install.ps1`
entirely:

```powershell
docker run -p 3001:3001 -v ctxone-data:/data ghcr.io/ctxone/ctxone:latest
```

Docker Desktop pulls the Linux multi-arch image (amd64 or arm64 depending
on your CPU) and runs it inside its built-in WSL2 Linux VM. From the
Windows side, you just see `localhost:3001`.

**Caveats with this path:**

- There's no `ctx.exe` on your Windows shell unless you also run
  `install.ps1`. You'd have to use the Hub via HTTP directly, or exec
  into the container (`docker exec -it <container> ctx ...`).
- `ctx init` can't write configs into the container's isolated
  filesystem. Install `ctx.exe` natively if you want auto-config.
- Most users pair Docker for the Hub with `install.ps1` for the CLI.

### Option 3: Build from source

You need Rust ≥1.88 and Git.

```powershell
git clone --recursive https://github.com/ctxone/ctxone.git
cd ctxone
cargo build --workspace --release
```

The binaries land in `target\release\ctx.exe` and
`target\release\ctxone-hub.exe`. Copy them anywhere on your PATH, or
cargo-install them:

```powershell
cargo install --path cli
cargo install --path server
```

## First run

After installing via `install.ps1`, open a **new** PowerShell window
and check everything is healthy:

```powershell
ctx --version
ctx doctor
```

`ctx doctor` will show ✗ next to "hub HTTP endpoint" until you start the
Hub. That's expected.

### Start the Hub

```powershell
ctx serve --http
```

You'll see:

```
Starting CtxOne Hub on port 3001 (db: C:\Users\you\AppData\Roaming\ctxone\memory.db)
CtxOne Hub v0.60.0
Storage: C:\Users\you\AppData\Roaming\ctxone\memory.db
HTTP API listening on http://0.0.0.0:3001
```

Leave this window running. Open a second PowerShell window for the rest.

### Verify the Hub is working

In the second window:

```powershell
ctx status
ctx demo
```

`ctx demo` seeds 21 facts and runs four recalls, showing live token
savings. If you see output like `18.4x overall`, everything's working.

## Wiring CtxOne into your AI tools

```powershell
ctx init
```

On Windows, `ctx init` detects and writes configs for:

| Tool | Config path |
|------|-------------|
| Claude Code (project) | `.\.mcp.json` |
| Claude Desktop | `%APPDATA%\Claude\claude_desktop_config.json` |
| Cursor | `.\.cursor\mcp.json` or `%USERPROFILE%\.cursor\mcp.json` |
| VS Code | `.\.vscode\mcp.json` or `%APPDATA%\Code\User\settings.json` |
| Codex | `%USERPROFILE%\.codex\config.toml` |
| Gemini | `.\.gemini\settings.json` or `%USERPROFILE%\.gemini\settings.json` |
| Grok | `.\.grok\settings.json` or `%USERPROFILE%\.grok\settings.json` |

By default, `ctx init` writes project-local configs in the current
directory. Use `ctx init --global` to write user-level configs.

After `ctx init`, **restart** each AI tool so it picks up the new MCP
server config.

## Running the Hub as a background service

Having a PowerShell window pinned to `ctx serve --http` works for
development. For daily use, there are three options to run it in the
background.

### Option A: Windows Task Scheduler (no admin required)

The cleanest solution. Register a task that runs at login.

```powershell
# Register the task once
$Action = New-ScheduledTaskAction `
    -Execute "$env:LOCALAPPDATA\ctxone\bin\ctxone-hub.exe" `
    -Argument "--http --port 3001 --path $env:APPDATA\ctxone\memory.db"

$Trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME

$Settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -StartWhenAvailable

Register-ScheduledTask `
    -TaskName "CtxOne Hub" `
    -Action $Action `
    -Trigger $Trigger `
    -Settings $Settings `
    -Description "CtxOne memory layer for AI agents"
```

Start it immediately:

```powershell
Start-ScheduledTask -TaskName "CtxOne Hub"
```

Verify:

```powershell
ctx status
```

Remove later:

```powershell
Unregister-ScheduledTask -TaskName "CtxOne Hub" -Confirm:$false
```

### Option B: Start menu shortcut

Lowest friction if you don't need it running all the time.

```powershell
$WScript = New-Object -ComObject WScript.Shell
$Shortcut = $WScript.CreateShortcut("$env:APPDATA\Microsoft\Windows\Start Menu\Programs\CtxOne Hub.lnk")
$Shortcut.TargetPath = "$env:LOCALAPPDATA\ctxone\bin\ctxone-hub.exe"
$Shortcut.Arguments = "--http --port 3001"
$Shortcut.WorkingDirectory = "$env:APPDATA\ctxone"
$Shortcut.WindowStyle = 7   # Minimized
$Shortcut.Save()
```

Now "CtxOne Hub" appears in your Start menu and launches a minimized
window when clicked.

### Option C: Run as a real Windows service (NSSM)

For always-on production-style setups. Install
[NSSM](https://nssm.cc/) first, then:

```powershell
nssm install CtxOneHub "$env:LOCALAPPDATA\ctxone\bin\ctxone-hub.exe"
nssm set CtxOneHub AppParameters "--http --port 3001 --path $env:APPDATA\ctxone\memory.db"
nssm set CtxOneHub DisplayName "CtxOne Hub"
nssm set CtxOneHub Start SERVICE_AUTO_START
nssm start CtxOneHub
```

This runs the Hub under the Windows service control manager — it starts
at boot before any user logs in, restarts on crash, and shows up in
`services.msc`.

## Paths reference

CtxOne uses Windows-standard locations:

| What | Where |
|------|-------|
| Binaries | `%LOCALAPPDATA%\ctxone\bin\` (e.g., `C:\Users\you\AppData\Local\ctxone\bin`) |
| Memory database | `%APPDATA%\ctxone\memory.db` (e.g., `C:\Users\you\AppData\Roaming\ctxone\memory.db`) |
| Config file | `%APPDATA%\ctxone\config.toml` |
| Log file | (Hub logs to stderr by default; redirect with `2>` if needed) |

## Updating

Re-run the installer. It overwrites `ctx.exe` and `ctxone-hub.exe` with
the latest release.

```powershell
iwr https://raw.githubusercontent.com/ctxone/ctxone/main/install.ps1 | iex
```

If the Hub is running via Task Scheduler or NSSM, restart it:

```powershell
# Task Scheduler
Stop-ScheduledTask -TaskName "CtxOne Hub"
Start-ScheduledTask -TaskName "CtxOne Hub"

# NSSM
nssm restart CtxOneHub
```

## Uninstalling

```powershell
# 1. Stop any running Hub
Stop-Process -Name "ctxone-hub" -ErrorAction SilentlyContinue

# 2. Remove scheduled task (if you set one up)
Unregister-ScheduledTask -TaskName "CtxOne Hub" -Confirm:$false -ErrorAction SilentlyContinue

# 3. Remove NSSM service (if you set one up)
nssm remove CtxOneHub confirm

# 4. Delete the binaries
Remove-Item -Recurse -Force "$env:LOCALAPPDATA\ctxone"

# 5. Delete the memory database and config (WARNING: this loses your memory)
Remove-Item -Recurse -Force "$env:APPDATA\ctxone"

# 6. Remove from user PATH (manual)
# Open: Start → Edit environment variables for your account
# Remove %LOCALAPPDATA%\ctxone\bin from the Path entry
```

## Troubleshooting

### `ctx: The term 'ctx' is not recognized`

Your current PowerShell session doesn't have the updated PATH. Close the
window and open a new PowerShell. If that still fails, check:

```powershell
Get-Item "$env:LOCALAPPDATA\ctxone\bin\ctx.exe"
$env:Path -split ';' | Select-String "ctxone"
```

If the binary is there but not on PATH, add it manually:

```powershell
$env:Path += ";$env:LOCALAPPDATA\ctxone\bin"
```

(Persistent across sessions only if you use `[Environment]::SetEnvironmentVariable`
— see `install.ps1` for the exact invocation.)

### Installer blocked by execution policy

```
File install.ps1 cannot be loaded because running scripts is disabled on
this system.
```

Use a process-scoped bypass:

```powershell
Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass
iwr https://raw.githubusercontent.com/ctxone/ctxone/main/install.ps1 | iex
```

This only affects the current window, not your system policy.

### Installer blocked by antivirus / Defender

Some AV products flag new .exe downloads from GitHub releases. If
Windows Defender quarantines `ctx.exe` or `ctxone-hub.exe`:

1. Open Windows Security → Virus & threat protection → Protection history
2. Find the quarantined file and choose "Restore"
3. Add `%LOCALAPPDATA%\ctxone\bin` to Defender's exclusions if it keeps
   happening

### `ctx serve` reports port 3001 in use

Something else is bound to 3001. Use a different port:

```powershell
ctx serve --http --port 3002
ctx --server http://localhost:3002 status
```

Or persist the new default in your config:

```powershell
ctx config set server http://localhost:3002
```

### `ctx init` wrote configs but Claude Code / Cursor can't see CtxOne

Restart the AI tool. Most MCP clients load config at startup. If the
tool still doesn't see CtxOne:

```powershell
ctx init --dry-run
```

Copy the reported config path, open it in a text editor, and verify
the `mcpServers.ctxone` entry points at your real `ctxone-hub.exe` path.

### Firewall warning on first run

Windows Defender Firewall may prompt when `ctxone-hub.exe` first binds
to port 3001. Allow it for **private networks** (you don't need public
network access — the Hub is for your local AI tools).

### The memory database is huge / I want to start over

```powershell
# Stop the Hub first
Stop-Process -Name "ctxone-hub" -ErrorAction SilentlyContinue

# Delete the database
Remove-Item "$env:APPDATA\ctxone\memory.db"

# Restart the Hub — it'll create a fresh database
ctx serve --http
```

Your config file stays intact; only the memory is wiped.

## Docker-on-Windows notes

If you're using Docker Desktop instead of `install.ps1`:

- Docker Desktop runs Linux containers via a WSL2-based Linux VM. Our
  `ghcr.io/ctxone/ctxone` image is a Linux image; it runs in that VM.
  You never see or touch the VM directly.
- `docker run -p 3001:3001 ...` forwards port 3001 from the VM to
  Windows localhost. `ctx status` (if you also installed `ctx.exe`)
  will connect to `http://localhost:3001` and just work.
- The container's filesystem is isolated. `ctx init` runs on Windows
  (not in the container) and writes configs to Windows paths. The
  container and the config paths are two different concerns — the
  container is the Hub, the Windows binary is the client.

A common setup:

- Hub runs in Docker Desktop (auto-starts with Docker)
- `ctx.exe` runs natively on Windows (from `install.ps1`)
- Both talk to `http://localhost:3001`

## Next steps

- [Quickstart](QUICKSTART.md) — the cross-platform 5-minute walkthrough
- [Architecture](ARCHITECTURE.md) — how recall and pinning work
- [Cookbook](COOKBOOK.md) — real workflow recipes (some assume bash;
  most translate to PowerShell with minor tweaks)
- [CLI Reference](CLI_REFERENCE.md) — every command and flag
- [Troubleshooting](TROUBLESHOOTING.md) — the full top-10 error list

# CtxOne Windows installer
#
# Usage:
#   Invoke-WebRequest https://raw.githubusercontent.com/AgentStateLabs/CTXone/main/install.ps1 | Invoke-Expression
#   # or, shorter:
#   iwr https://raw.githubusercontent.com/AgentStateLabs/CTXone/main/install.ps1 | iex
#
# Downloads ctx.exe and ctxone-hub.exe from the latest GitHub release and
# drops them in %LOCALAPPDATA%\ctxone\bin, which it adds to your PATH.

$ErrorActionPreference = "Stop"

$Repo = "ctxone/ctxone"
$InstallDir = Join-Path $env:LOCALAPPDATA "ctxone\bin"

Write-Host "CtxOne installer (Windows)" -ForegroundColor Cyan
Write-Host ""

# Detect architecture
$Arch = switch ($env:PROCESSOR_ARCHITECTURE) {
    "AMD64" { "x86_64" }
    "ARM64" {
        Write-Host "Note: ARM64 Windows isn't a release target yet." -ForegroundColor Yellow
        Write-Host "The x86_64 binaries should run under Windows ARM emulation." -ForegroundColor Yellow
        "x86_64"
    }
    default {
        Write-Error "Unsupported architecture: $env:PROCESSOR_ARCHITECTURE"
        exit 1
    }
}

$Target = "$Arch-pc-windows-msvc"
Write-Host "  Target: $Target"
Write-Host "  Dir:    $InstallDir"
Write-Host ""

# Create the install directory
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

# Get the latest release tag from the GitHub API
Write-Host "Fetching latest release..."
try {
    $Latest = (Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest").tag_name
} catch {
    Write-Error "Could not fetch latest release. Check your network connection."
    exit 1
}

if (-not $Latest) {
    Write-Host "No releases found. To build from source:" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "  git clone --recursive https://github.com/$Repo.git"
    Write-Host "  cd ctxone"
    Write-Host "  cargo build --workspace --release"
    exit 1
}

Write-Host "Installing CtxOne $Latest..."
Write-Host ""

# Download each binary
foreach ($bin in @("ctx", "ctxone-hub")) {
    $Url = "https://github.com/$Repo/releases/download/$Latest/$bin-$Target.exe"
    $Dest = Join-Path $InstallDir "$bin.exe"
    Write-Host "  Downloading $bin.exe..."
    try {
        Invoke-WebRequest -Uri $Url -OutFile $Dest -UseBasicParsing
    } catch {
        Write-Error "Failed to download $Url"
        Write-Host "  If this version is too old to include Windows binaries, build from source." -ForegroundColor Yellow
        exit 1
    }
}

Write-Host ""
Write-Host "Installed to $InstallDir" -ForegroundColor Green

# Add to PATH (user-level, no admin required)
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not $UserPath) { $UserPath = "" }
$PathEntries = $UserPath -split ';' | Where-Object { $_ -ne "" }

if ($PathEntries -notcontains $InstallDir) {
    $NewPath = ($PathEntries + $InstallDir) -join ';'
    [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
    Write-Host ""
    Write-Host "Added $InstallDir to your user PATH." -ForegroundColor Green
    Write-Host "Open a new PowerShell window for it to take effect." -ForegroundColor Yellow
} else {
    Write-Host "(already on PATH)" -ForegroundColor DarkGray
}

Write-Host ""
Write-Host "Get started:" -ForegroundColor Cyan
Write-Host "  ctx init          # Configure your AI tools"
Write-Host "  ctx serve --http  # Start the Hub"
Write-Host "  ctx demo          # See live token savings"
Write-Host ""
Write-Host "Docs: https://github.com/AgentStateLabs/CTXone#readme"

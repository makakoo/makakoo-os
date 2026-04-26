<#
.SYNOPSIS
Makakoo OS installer — Windows

.DESCRIPTION
Downloads the Makakoo `makakoo.exe` binary from GitHub releases and
places it under %LOCALAPPDATA%\Makakoo\bin. Refuses to run unless
Windows Developer Mode is enabled — Makakoo needs unprivileged symlink
creation for infecting AI CLI host config directories.

After the binary is on disk, finish setup with:
    makakoo install

.PARAMETER Version
Release to install. Defaults to `latest`. Accepts both `0.1.0` and
`v0.1.0`.

.PARAMETER InstallDir
Directory to place `makakoo.exe`. Defaults to
`%LOCALAPPDATA%\Makakoo\bin`.

.PARAMETER DryRun
Print the plan without downloading or installing.

.EXAMPLE
iwr -UseBasicParsing https://makakoo.com/install.ps1 | iex

.EXAMPLE
$env:MAKAKOO_VERSION = "0.1.0"
iwr -UseBasicParsing https://makakoo.com/install.ps1 | iex
#>

[CmdletBinding()]
param(
    [string]$Version = $env:MAKAKOO_VERSION,
    [string]$InstallDir = $env:MAKAKOO_INSTALL_DIR,
    [string]$Repo = $env:MAKAKOO_REPO,
    [string]$LocalTarball = $env:MAKAKOO_LOCAL_TARBALL,
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"

if (-not $Version)    { $Version    = "latest" }
if (-not $InstallDir) { $InstallDir = Join-Path $env:LOCALAPPDATA "Makakoo\bin" }
if (-not $Repo)       { $Repo       = "makakoo/makakoo-os" }

# ─── Dev Mode check ──────────────────────────────────────────────────

function Test-DevMode {
    try {
        $key = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock"
        $val = Get-ItemProperty -Path $key -Name "AllowDevelopmentWithoutDevLicense" -ErrorAction Stop
        return ($val.AllowDevelopmentWithoutDevLicense -eq 1)
    } catch {
        return $false
    }
}

if (-not (Test-DevMode)) {
    Write-Error @"
Makakoo requires Windows Developer Mode.

Infect step creates symlinks from AI CLI config directories into
`$env:LOCALAPPDATA\Makakoo`, which needs unprivileged symlink
permission. Enable Developer Mode:

    Settings → Privacy & security → For developers → Developer Mode → On

Then re-run this installer.
"@
    exit 1
}

# ─── detect target triple ────────────────────────────────────────────

$arch = $env:PROCESSOR_ARCHITECTURE
switch ($arch) {
    "AMD64" { $target = "x86_64-pc-windows-msvc" }
    "ARM64" { $target = "aarch64-pc-windows-msvc" }
    default { Write-Error "unsupported arch: $arch"; exit 1 }
}

# ─── resolve URL ─────────────────────────────────────────────────────

$v = $Version
if ($v -ne "latest" -and -not $v.StartsWith("v")) { $v = "v$v" }

if ($v -eq "latest") {
    $zipUrl = "https://github.com/$Repo/releases/latest/download/makakoo-$target.zip"
} else {
    $zipUrl = "https://github.com/$Repo/releases/download/$v/makakoo-$target.zip"
}

# ─── print plan ──────────────────────────────────────────────────────

Write-Host "Makakoo OS installer"
Write-Host "  target:       $target"
Write-Host "  version:      $Version"
Write-Host "  install dir:  $InstallDir"
if ($LocalTarball) {
    Write-Host "  source:       $LocalTarball  (local archive, skipping download)"
} else {
    Write-Host "  source:       $zipUrl"
}

if ($DryRun) {
    Write-Host ""
    Write-Host "--DryRun: no changes made"
    exit 0
}

# ─── download + extract ──────────────────────────────────────────────

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("makakoo-install-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tmp -Force | Out-Null

try {
    $archive = Join-Path $tmp "makakoo.zip"

    if ($LocalTarball) {
        if (-not (Test-Path -LiteralPath $LocalTarball)) {
            Write-Error "local archive not found: $LocalTarball"
            exit 1
        }
        Copy-Item -LiteralPath $LocalTarball -Destination $archive
    } else {
        Write-Host ""
        Write-Host "downloading…"
        Invoke-WebRequest -Uri $zipUrl -OutFile $archive -UseBasicParsing
    }

    Write-Host "extracting…"
    Expand-Archive -Path $archive -DestinationPath $tmp -Force

    $binSrc = Get-ChildItem -Path $tmp -Filter "makakoo.exe" -Recurse | Select-Object -First 1
    if (-not $binSrc) {
        Write-Error "makakoo.exe not found in downloaded archive"
        exit 1
    }
    $stageDir = $binSrc.Directory.FullName

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    $binDst = Join-Path $InstallDir "makakoo.exe"
    Move-Item -LiteralPath $binSrc.FullName -Destination $binDst -Force

    # makakoo-mcp.exe lives next to makakoo.exe in the zip — install it too.
    $mcpSrc = Join-Path $stageDir "makakoo-mcp.exe"
    if (Test-Path -LiteralPath $mcpSrc) {
        $mcpDst = Join-Path $InstallDir "makakoo-mcp.exe"
        Move-Item -LiteralPath $mcpSrc -Destination $mcpDst -Force
    }

    # Bundled runtime data: distros/ + plugins-core/ extract to
    # <InstallDir>\..\share\makakoo\. The binary's resolve_distros_dir()
    # and plugins_core_root() walk <exe>/../share/makakoo/ as a fallback.
    $shareRoot = Join-Path (Split-Path -Parent $InstallDir) "share\makakoo"
    New-Item -ItemType Directory -Path $shareRoot -Force | Out-Null
    foreach ($dir in @("distros", "plugins-core")) {
        $src = Join-Path $stageDir $dir
        if (Test-Path -LiteralPath $src) {
            $dst = Join-Path $shareRoot $dir
            if (Test-Path -LiteralPath $dst) { Remove-Item -LiteralPath $dst -Recurse -Force }
            Copy-Item -LiteralPath $src -Destination $dst -Recurse -Force
        }
    }

    Write-Host ""
    Write-Host "installed: $binDst"
    Write-Host "bundled distros + plugins-core: $shareRoot"

    # ─── PATH hint ───────────────────────────────────────────────────
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($userPath -notlike "*$InstallDir*") {
        Write-Host ""
        Write-Host "note: $InstallDir is not on your user PATH."
        Write-Host "add it with:"
        Write-Host "    [Environment]::SetEnvironmentVariable('Path', `"`$env:Path;$InstallDir`", 'User')"
    }

    Write-Host ""
    Write-Host "next step:"
    Write-Host "    $binDst install"
    Write-Host ""
    Write-Host "This runs the core distro install, registers the Task Scheduler"
    Write-Host "agent, and infects every detected AI CLI host with the Makakoo"
    Write-Host "bootstrap block."
}
finally {
    Remove-Item -Path $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

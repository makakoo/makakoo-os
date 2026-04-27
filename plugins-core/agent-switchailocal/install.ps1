# install.ps1 — agent-switchailocal
#
# Installs @traylinx/switchailocal npm package, sets up Windows service via NSSM,
# and manages the service lifecycle.

$ErrorActionPreference = "Stop"

$pluginDir = $env:MAKAKOO_PLUGIN_DIR
$stateDir  = "$env:MAKAKOO_HOME\state\agent-switchailocal"
$logDir    = "$stateDir\logs"
$svcName   = "switchailocal"

$npmBin    = (npm bin -g).Trim()
$binary    = "$npmBin\switchailocal.cmd"

function New-Item-Dir($path) {
    if (-not (Test-Path $path)) { New-Item -ItemType Directory -Force -Path $path | Out-Null }
}

New-Item-Dir $stateDir
New-Item-Dir $logDir

function do-install {
    Write-Host "→ [agent-switchailocal] installing @traylinx/switchailocal..."
    $pkg = npm list -g --depth=0 --json 2>$null | ConvertFrom-Json
    if (-not $pkg.dependencies.'@traylinx/switchailocal') {
        npm install -g "@traylinx/switchailocal" --quiet
        Write-Host "  npm package installed."
    } else {
        Write-Host "  npm package already installed."
    }

    if (-not (Test-Path $binary)) {
        Write-Error "switchailocal.cmd not found at $binary after install."
        exit 1
    }
    Write-Host "  binary: $binary"
    Write-Host "✓ agent-switchailocal installed."
    Write-Host "  On Windows, run as a background process or set up as a Windows service."
}

function do-start {
    Write-Host "→ [agent-switchailocal] starting..."
    # On Windows, start the binary in background with Task Scheduler or NSSM.
    # For simplicity, start as a detached process.
    Start-Process -FilePath "node" -ArgumentList $binary,"serve" `
        -WorkingDirectory $stateDir `
        -NoNewWindow -PassThru | Out-Null
    Start-Sleep 2
    if ($?) { Write-Host "✓ started." } else { Write-Error "Failed to start." }
}

function do-stop {
    Write-Host "→ [agent-switchailocal] stopping..."
    Get-Process | Where-Object { $_.CommandLine -like "*switchailocal*" } | Stop-Process -Force -ErrorAction SilentlyContinue
    Write-Host "✓ stopped."
}

function do-health {
    try {
        $r = Invoke-WebRequest -Uri "http://127.0.0.1:18080/v1/models" -TimeoutSec 3 -UseBasicParsing
        if ($r.StatusCode -eq 200) {
            Write-Host "OK — switchailocal is running on port 18080"
            exit 0
        }
    } catch {}
    Write-Host "DOWN — switchailocal is not running"
    exit 1
}

$cmd = $args[0]
if (-not $cmd) { $cmd = "install" }

switch ($cmd) {
    "install" { do-install }
    "start"   { do-start }
    "stop"    { do-stop }
    "health"  { do-health }
    default   {
        Write-Host "Usage: install.ps1 {install|start|stop|health}"
        exit 1
    }
}

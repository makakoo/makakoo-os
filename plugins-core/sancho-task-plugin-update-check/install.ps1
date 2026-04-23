# install.ps1 — sancho-task-plugin-update-check
$ErrorActionPreference = "Stop"

$stateDir = Join-Path $env:MAKAKOO_HOME "state/sancho-task-plugin-update-check"
$config = Join-Path $stateDir "plugins-to-update.json"

if (-not (Test-Path $config)) {
    New-Item -ItemType Directory -Force -Path $stateDir | Out-Null
    Copy-Item -Path (Join-Path $MAKAKOO_PLUGIN_DIR "default-config/plugins-to-update.json") -Destination $config
    Write-Host "→ [sancho-task-plugin-update-check] seeded config at ${config}"
} else {
    Write-Host "→ [sancho-task-plugin-update-check] config already exists — leaving untouched."
}

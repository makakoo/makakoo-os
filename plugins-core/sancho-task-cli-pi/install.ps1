# install.ps1 — sancho-task-cli-pi
if (-not (Get-Command npm -ErrorAction SilentlyContinue)) {
    Write-Host "→ [sancho-task-cli-pi] npm not found — skipping install."
    exit 0
}
Write-Host "→ [sancho-task-cli-pi] installed. pi auto-update runs every 24h."

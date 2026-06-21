# install.ps1 — sancho-task-makakoo-update
if (-not (Get-Command makakoo -ErrorAction SilentlyContinue)) {
    Write-Host "→ [sancho-task-makakoo-update] makakoo not found — skipping install."
    exit 0
}
Write-Host "→ [sancho-task-makakoo-update] installed. Makakoo OS auto-update runs every 24h when config/updates.toml mode=auto."

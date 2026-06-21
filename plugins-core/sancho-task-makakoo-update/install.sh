#!/usr/bin/env bash
# install.sh — sancho-task-makakoo-update
set -euo pipefail

if ! command -v makakoo >/dev/null 2>&1; then
    echo "→ [sancho-task-makakoo-update] makakoo not found — skipping install."
    exit 0
fi

echo "→ [sancho-task-makakoo-update] installed. Makakoo OS auto-update runs every 24h when config/updates.toml mode=auto."

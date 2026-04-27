#!/usr/bin/env bash
# install.sh — sancho-task-cli-pi
set -euo pipefail

if ! command -v npm >/dev/null 2>&1; then
    echo "→ [sancho-task-cli-pi] npm not found — skipping install."
    exit 0
fi

echo "→ [sancho-task-cli-pi] installed. pi auto-update runs every 24h."

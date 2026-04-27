#!/usr/bin/env bash
# install.sh — sancho-task-cli-ghostty
set -euo pipefail

if [[ "$(uname)" != "Darwin" ]]; then
    echo "→ [sancho-task-cli-ghostty] not on macOS — skipping install (Ghostty is macOS-only)."
    exit 0
fi

if ! command -v brew >/dev/null 2>&1; then
    echo "→ [sancho-task-cli-ghostty] Homebrew not found — skipping install."
    exit 0
fi

echo "→ [sancho-task-cli-ghostty] installed. Ghostty auto-update runs every 24h on macOS."

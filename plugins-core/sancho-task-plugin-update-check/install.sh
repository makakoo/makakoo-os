#!/usr/bin/env bash
# install.sh — sancho-task-plugin-update-check
set -euo pipefail

# Copy default plugins-to-update.json to state dir on first install.
STATE_DIR="${MAKAKOO_HOME}/state/sancho-task-plugin-update-check"
CONFIG="${STATE_DIR}/plugins-to-update.json"

if [[ ! -f "${CONFIG}" ]]; then
    mkdir -p "${STATE_DIR}"
    cp "${MAKAKOO_PLUGIN_DIR}/default-config/plugins-to-update.json" "${CONFIG}"
    echo "→ [sancho-task-plugin-update-check] seeded config at ${CONFIG}"
    echo "  Edit this file to add more auto-update plugins or disable browser-harness."
else
    echo "→ [sancho-task-plugin-update-check] config already exists at ${CONFIG} — leaving untouched."
fi

#!/usr/bin/env bash
# install.sh — bootstrap the HarveyChat/Telegram agent plugin.

set -euo pipefail

PYTHON_BIN="${MAKAKOO_VENV_PYTHON:-}"
if [[ -z "${PYTHON_BIN}" ]]; then
    for candidate in python3.13 python3.12 python3.11 python3; do
        if command -v "${candidate}" >/dev/null 2>&1 && "${candidate}" - <<'PYVERSION' >/dev/null 2>&1
import sys
raise SystemExit(0 if sys.version_info >= (3, 9) else 1)
PYVERSION
        then
            PYTHON_BIN="${candidate}"
            break
        fi
    done
fi

if [[ -z "${PYTHON_BIN}" ]]; then
    cat <<'NOTE'
    ⚠ agent-harveychat runtime skipped: Python >=3.9 not found.
      Install Python and re-run:
        makakoo plugin install --core agent-harveychat
NOTE
    exit 0
fi

echo "→ [agent-harveychat] bootstrapping venv (${PYTHON_BIN})"
"${PYTHON_BIN}" -m venv "${MAKAKOO_PLUGIN_DIR}/.venv"
"${MAKAKOO_PLUGIN_DIR}/.venv/bin/python" -m pip install --upgrade pip >/dev/null

echo "→ [agent-harveychat] installing Telegram/chat dependencies"
"${MAKAKOO_PLUGIN_DIR}/.venv/bin/python" -m pip install -r "${MAKAKOO_PLUGIN_DIR}/requirements.txt"

cat <<'NOTE'
✓ agent-harveychat installed.
  Configure TELEGRAM_BOT_TOKEN + SWITCHAI_KEY, then run:
    makakoo agent start agent-harveychat
NOTE

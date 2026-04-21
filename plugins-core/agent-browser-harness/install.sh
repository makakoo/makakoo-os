#!/usr/bin/env bash
# install.sh — bootstrap the agent-browser-harness plugin.
#
# CWD = $MAKAKOO_HOME/plugins/agent-browser-harness/ (set by the installer).
# $MAKAKOO_PLUGIN_DIR + $MAKAKOO_HOME are exported by the Rust installer.
#
# Steps:
#   1. Shallow-clone github.com/browser-use/browser-harness into upstream/
#      (honors BROWSER_HARNESS_UPSTREAM + BROWSER_HARNESS_REF overrides).
#   2. Bootstrap a per-plugin venv and pip install -e upstream/.
#   3. Run the doctor — warn (not fail) if Chrome with CDP isn't reachable.

set -euo pipefail

UPSTREAM_URL="${BROWSER_HARNESS_UPSTREAM:-https://github.com/browser-use/browser-harness}"
UPSTREAM_REF="${BROWSER_HARNESS_REF:-main}"
UPSTREAM_DIR="${MAKAKOO_PLUGIN_DIR}/upstream"

echo "→ [agent-browser-harness] ensuring upstream clone at ${UPSTREAM_REF}"
if [[ -d "${UPSTREAM_DIR}/.git" ]]; then
    git -C "${UPSTREAM_DIR}" fetch --depth 1 origin "${UPSTREAM_REF}" >/dev/null
    git -C "${UPSTREAM_DIR}" checkout -q FETCH_HEAD
else
    git clone --quiet --depth 1 --branch "${UPSTREAM_REF}" "${UPSTREAM_URL}" "${UPSTREAM_DIR}"
fi

echo "→ [agent-browser-harness] bootstrapping venv + pip install -e upstream/"
# Pass the editable target via --spec so `pip install -e <dir>` runs.
makakoo-venv-bootstrap pip "-e ${UPSTREAM_DIR}"

echo "→ [agent-browser-harness] running doctor (non-fatal)"
if ! "${MAKAKOO_PLUGIN_DIR}/.venv/bin/python" "${MAKAKOO_PLUGIN_DIR}/daemon_admin.py" doctor; then
    cat <<'NOTE'
    ⚠ Chrome with CDP port 9222 not reachable.
      Start your local Chrome with:
        google-chrome --remote-debugging-port=9222 --user-data-dir=/tmp/chrome-cdp
      (or the equivalent for Edge / Chromium). See
      docs/plugins/browser-harness.md for the full setup.
NOTE
fi

echo "✓ agent-browser-harness installed. Start with: makakoo agent start agent-browser-harness"

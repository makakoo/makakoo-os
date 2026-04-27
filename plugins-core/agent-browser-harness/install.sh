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
UPSTREAM_REF="${BROWSER_HARNESS_REF:-}"
UPSTREAM_DIR="${MAKAKOO_PLUGIN_DIR}/upstream"

# Resolve UPSTREAM_REF: use env override if set, otherwise fetch the
# latest browser-use/browser-harness GitHub release tag.
# Falls back to 'main' only if network is unavailable.
_resolve_ref() {
    if [[ -n "${UPSTREAM_REF}" ]]; then
        echo "${UPSTREAM_REF}"
        return
    fi
    if command -v gh >/dev/null 2>&1; then
        local tag
        tag=$(gh api repos/browser-use/browser-harness/releases/latest --jq '.tag_name' 2>/dev/null) && \
        [[ -n "${tag}" ]] && { echo "${tag}"; return; }
    fi
    # Fallback: parse GitHub tags page (works without gh CLI)
    local tag
    tag=$(curl -sSL --fail "https://api.github.com/repos/browser-use/browser-harness/releases/latest" \
        -H "Accept: application/vnd.github+json" \
        --max-time 10 2>/dev/null | grep '"tag_name"' | sed 's/.*": *"\([^"]*\)".*/\1/')
    if [[ -n "${tag}" ]]; then
        echo "${tag}"
        return
    fi
    echo "main"  # last resort
}

UPSTREAM_REF=$(_resolve_ref)
echo "→ [agent-browser-harness] upstream ref resolved to: ${UPSTREAM_REF}"
echo "→ [agent-browser-harness] ensuring upstream clone"
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

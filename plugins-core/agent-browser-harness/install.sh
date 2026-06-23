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
# browser-harness currently requires Python >=3.11. macOS system python can
# still be 3.9, so pick a compatible interpreter when present. If none is
# available, keep the wrapper installed and let `makakoo agent doctor` explain
# the missing runtime instead of failing the whole Makakoo core distro.
PYTHON_BIN="${MAKAKOO_VENV_PYTHON:-}"
if [[ -z "${PYTHON_BIN}" ]]; then
    for candidate in python3.13 python3.12 python3.11 python3; do
        if command -v "${candidate}" >/dev/null 2>&1 && "${candidate}" - <<'PYVERSION' >/dev/null 2>&1
import sys
raise SystemExit(0 if sys.version_info >= (3, 11) else 1)
PYVERSION
        then
            PYTHON_BIN="${candidate}"
            break
        fi
    done
fi

if [[ -z "${PYTHON_BIN}" ]]; then
    cat <<'NOTE'
    ⚠ agent-browser-harness runtime skipped: Python >=3.11 not found.
      The wrapper is installed, but the browser harness venv was not created.
      Install Python 3.11+ and re-run:
        makakoo plugin install --core agent-browser-harness
NOTE
    exit 0
fi

export MAKAKOO_VENV_PYTHON="${PYTHON_BIN}"
# Pass the editable target via --spec so `pip install -e <dir>` runs.
makakoo-venv-bootstrap pip "-e ${UPSTREAM_DIR}"

echo "→ [agent-browser-harness] writing compatibility shims"
# Older Makakoo MCP/agent wrappers looked for flat upstream/run.py and
# upstream/admin.py. browser-harness v0.1.3 is packaged under
# src/browser_harness/. Keep tiny shims so existing MCP stdio children keep
# working until their binary/session is refreshed.
cat >"${UPSTREAM_DIR}/run.py" <<'PY'
"""Compatibility shim for Makakoo wrappers expecting upstream/run.py."""
from __future__ import annotations

import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
os.environ.setdefault("BH_AGENT_WORKSPACE", str(ROOT / "agent-workspace"))
sys.path.insert(0, str(ROOT / "src"))

from browser_harness.run import main  # noqa: E402

if __name__ == "__main__":
    main()
PY

cat >"${UPSTREAM_DIR}/admin.py" <<'PY'
"""Compatibility shim for Makakoo wrappers expecting upstream/admin.py."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "src"))

from browser_harness.admin import *  # noqa: F401,F403,E402
PY

AGENT_HELPERS="${UPSTREAM_DIR}/agent-workspace/agent_helpers.py"
mkdir -p "$(dirname "${AGENT_HELPERS}")"
touch "${AGENT_HELPERS}"
python3 - "${AGENT_HELPERS}" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
start = "# --- Makakoo compatibility aliases START ---"
end = "# --- Makakoo compatibility aliases END ---"
block = """# --- Makakoo compatibility aliases START ---
# Makakoo's global bootstrap historically promised concise helpers named
# goto/read/click/fill/screenshot. browser-harness v0.1.3 exposes the lower-level
# names goto_url/click_at_xy/fill_input/capture_screenshot. Keep the old names
# here so existing CLI prompts and MCP snippets do not break on upstream drift.
def goto(url, wait_for=True, timeout=15.0):
    from browser_harness.helpers import goto_url, wait_for_load
    result = goto_url(url)
    if wait_for:
        wait_for_load(timeout=timeout)
    return result


def read(selector="body", max_chars=None):
    import json
    from browser_harness.helpers import js
    text = js(
        "(()=>{const e=document.querySelector("
        + json.dumps(selector)
        + "); return e ? (e.innerText || e.textContent || '') : '';})()"
    ) or ""
    return text[:max_chars] if max_chars else text


def click(selector_or_x, y=None, button="left", clicks=1, timeout=0.0):
    import json
    from browser_harness.helpers import click_at_xy, js, wait_for_element
    if y is not None:
        return click_at_xy(selector_or_x, y, button=button, clicks=clicks)
    selector = selector_or_x
    if timeout:
        wait_for_element(selector, timeout=timeout, visible=True)
    rect = js(
        "(()=>{const e=document.querySelector("
        + json.dumps(selector)
        + "); if(!e)return null; const r=e.getBoundingClientRect();"
        + "return {x:r.left+r.width/2,y:r.top+r.height/2};})()"
    )
    if not rect:
        raise RuntimeError(f"click: element not found: {selector!r}")
    return click_at_xy(rect["x"], rect["y"], button=button, clicks=clicks)


def fill(selector, text, **kwargs):
    from browser_harness.helpers import fill_input
    return fill_input(selector, text, **kwargs)


def screenshot(path=None, full=False, max_dim=None):
    from browser_harness.helpers import capture_screenshot
    return capture_screenshot(path=path, full=full, max_dim=max_dim)
# --- Makakoo compatibility aliases END ---
"""

text = path.read_text() if path.exists() else ""
if start in text and end in text:
    before, rest = text.split(start, 1)
    _, after = rest.split(end, 1)
    text = before.rstrip() + "\n\n" + block + after
else:
    text = text.rstrip() + "\n\n" + block + "\n"
path.write_text(text)
PY

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

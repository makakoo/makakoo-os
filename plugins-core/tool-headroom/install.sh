#!/usr/bin/env bash
# Install Headroom's MCP surface for Makakoo OS hosts.
# Best-effort by design: a missing Python 3.10+ or offline PyPI must not break
# a fresh Makakoo distro install. Re-run this script after fixing the host.

set -u

log() { printf '[tool-headroom] %s\n' "$*"; }
warn() { printf '[tool-headroom] WARNING: %s\n' "$*" >&2; }

want_mcp="${MAKAKOO_HEADROOM_MCP_INSTALL:-1}"
proxy_url="${HEADROOM_PROXY_URL:-http://127.0.0.1:8787}"
agent_filter="${MAKAKOO_HEADROOM_AGENT:-}"

find_python() {
  for candidate in python3.12 python3.11 python3.10 python3; do
    command -v "$candidate" >/dev/null 2>&1 || continue
    if "$candidate" - <<'PY' >/dev/null 2>&1
import sys
raise SystemExit(0 if sys.version_info >= (3, 10) else 1)
PY
    then
      command -v "$candidate"
      return 0
    fi
  done
  return 1
}

ensure_headroom() {
  if command -v headroom >/dev/null 2>&1; then
    log "headroom already on PATH: $(command -v headroom)"
    headroom --version 2>/dev/null || true
    return 0
  fi

  pybin="$(find_python || true)"
  if [ -z "${pybin:-}" ]; then
    warn "Python 3.10+ not found; Headroom requires Python >=3.10. Skipping install."
    return 1
  fi

  if command -v pipx >/dev/null 2>&1; then
    log "installing headroom-ai[mcp] with pipx using $pybin"
    if pipx install --python "$pybin" 'headroom-ai[mcp]>=0.25.0'; then
      export PATH="$HOME/.local/bin:$PATH"
      return 0
    fi
    warn "pipx install failed; trying user-site pip fallback"
  fi

  log "installing headroom-ai[mcp] with $pybin -m pip --user"
  "$pybin" -m pip install --user 'headroom-ai[mcp]>=0.25.0' || return 1
  user_base="$($pybin - <<'PY' 2>/dev/null || true
import site
print(site.USER_BASE)
PY
)"
  if [ -n "${user_base:-}" ]; then
    export PATH="$user_base/bin:$HOME/.local/bin:$PATH"
  else
    export PATH="$HOME/.local/bin:$PATH"
  fi
}

register_mcp() {
  command -v headroom >/dev/null 2>&1 || return 1
  [ "$want_mcp" = "0" ] && { log "MCP registration skipped by MAKAKOO_HEADROOM_MCP_INSTALL=0"; return 0; }

  if [ -n "$agent_filter" ]; then
    log "registering Headroom MCP for agent '$agent_filter' at $proxy_url"
    headroom mcp install --agent "$agent_filter" --proxy-url "$proxy_url" || return 1
  else
    log "registering Headroom MCP for every detected agent at $proxy_url"
    headroom mcp install --proxy-url "$proxy_url" || return 1
  fi
}

if ensure_headroom; then
  if register_mcp; then
    log "Headroom MCP ready. Restart Claude/Codex/Cursor sessions to pick it up."
  else
    warn "Headroom installed but MCP registration failed. Run: headroom mcp install --proxy-url '$proxy_url'"
  fi
else
  warn "Headroom not installed. Install later with: pipx install --python python3.11 'headroom-ai[mcp]>=0.25.0' && headroom mcp install"
fi

# Install scripts are best-effort; never block a fresh Makakoo OS install.
exit 0

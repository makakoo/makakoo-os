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
docker_install_url="${HEADROOM_DOCKER_INSTALL_URL:-https://raw.githubusercontent.com/chopratejas/headroom/main/scripts/install.sh}"
headroom_docker_native=0
script_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"

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

is_headroom_docker_native() {
  hr="$(command -v headroom 2>/dev/null || true)"
  [ -n "${hr:-}" ] || return 1
  grep -q 'HEADROOM_IMAGE_DEFAULT' "$hr" 2>/dev/null
}

find_modern_bash() {
  for candidate in "${BASH:-}" bash /usr/local/bin/bash /opt/homebrew/bin/bash; do
    [ -n "$candidate" ] || continue
    if [ -x "$candidate" ]; then
      bashbin="$candidate"
    else
      bashbin="$(command -v "$candidate" 2>/dev/null || true)"
    fi
    [ -n "${bashbin:-}" ] || continue
    if "$bashbin" -c '((BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 3)))' >/dev/null 2>&1; then
      printf '%s\n' "$bashbin"
      return 0
    fi
  done
  return 1
}

install_headroom_docker_native() {
  command -v docker >/dev/null 2>&1 || { warn "Docker not found; Docker-native Headroom fallback unavailable."; return 1; }
  docker version >/dev/null 2>&1 || { warn "Docker is installed but not available; Docker-native Headroom fallback unavailable."; return 1; }
  command -v curl >/dev/null 2>&1 || { warn "curl not found; Docker-native Headroom fallback unavailable."; return 1; }

  bashbin="$(find_modern_bash || true)"
  [ -n "${bashbin:-}" ] || { warn "Bash >=4.3 not found; Docker-native Headroom fallback unavailable."; return 1; }

  tmp="${TMPDIR:-/tmp}/headroom-install.$$"
  rm -f "$tmp"
  log "installing Headroom Docker-native wrapper via $docker_install_url"
  if ! curl -fsSL "$docker_install_url" -o "$tmp"; then
    rm -f "$tmp"
    return 1
  fi
  if ! "$bashbin" "$tmp"; then
    rm -f "$tmp"
    return 1
  fi
  rm -f "$tmp"
  export PATH="$HOME/.local/bin:$HOME/bin:$PATH"
  if command -v headroom >/dev/null 2>&1; then
    headroom_docker_native=1
    return 0
  fi
  return 1
}

write_docker_native_mcp_shim() {
  mkdir -p "$HOME/.local/bin"
  shim="$HOME/.local/bin/headroom-mcp-stdio"
  cat >"$shim" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

# Ensure GUI-launched CLIs with a stripped PATH can still resolve docker.
export PATH="${PATH:-/usr/bin:/bin}:/usr/local/bin:/opt/homebrew/bin"

# Follow the locally installed Docker wrapper's pinned image without sourcing it.
# Explicit HEADROOM_DOCKER_IMAGE remains the highest-priority override.
image_default=ghcr.io/chopratejas/headroom:latest
wrapper_dir="$(cd -- "$(dirname -- "$0")" && pwd)"
if [[ -r "${wrapper_dir}/headroom" ]]; then
  candidate="$(sed -n 's/^HEADROOM_IMAGE_DEFAULT=//p' "${wrapper_dir}/headroom" | head -n 1)"
  candidate="${candidate#\"}"; candidate="${candidate%\"}"
  candidate="${candidate#\'}"; candidate="${candidate%\'}"
  # Only literal Docker image references, never shell expressions.
  if [[ "$candidate" =~ ^[a-zA-Z0-9][a-zA-Z0-9._/:@-]*$ ]]; then
    image_default="$candidate"
  fi
fi
HEADROOM_IMAGE="${HEADROOM_DOCKER_IMAGE:-$image_default}"
HEADROOM_CONTAINER_HOME="${HEADROOM_CONTAINER_HOME:-/tmp/headroom-home}"
HEADROOM_HOST_HOME="${HOME:?}"
HEADROOM_CONTAINER_NAME="${HEADROOM_CONTAINER_NAME:-headroom-default}"
# Loopback in host MCP configuration refers to the host, not a sidecar container.
container_proxy_url() {
  local url="$1"
  if [[ "$url" =~ ^(https?://)(localhost|127\.0\.0\.1|\[::1\])([:/?#].*|$) ]]; then
    printf '%s' "${BASH_REMATCH[1]}host.docker.internal${BASH_REMATCH[3]}"
  else
    printf '%s' "$url"
  fi
}
# Inside the persistent proxy container itself, direct loopback avoids the host bridge hop.
exec_proxy_url() {
  local url="$1"
  if [[ "$url" =~ ^(http://)(localhost|127\.0\.0\.1|\[::1\]|host\.docker\.internal):8787([/?#].*|$) ]]; then
    printf '%s' "http://127.0.0.1:8787${BASH_REMATCH[3]}"
  else
    container_proxy_url "$url"
  fi
}
raw_proxy_env="${HEADROOM_PROXY_URL:-http://127.0.0.1:8787}"
HEADROOM_PROXY_URL_VALUE="$(container_proxy_url "${raw_proxy_env}")"
HEADROOM_EXEC_PROXY_URL_VALUE="$(exec_proxy_url "${raw_proxy_env}")"
# CLI options override the environment, so normalize both forms as well.
mcp_args=()
exec_mcp_args=()
while (( $# )); do
  case "$1" in
    --proxy-url)
      if (( $# < 2 )); then
        printf '%s\n' 'headroom-mcp-stdio: --proxy-url requires a value' >&2
        exit 2
      fi
      mcp_args+=("$1" "$(container_proxy_url "$2")")
      exec_mcp_args+=("$1" "$(exec_proxy_url "$2")")
      shift 2 ;;
    --proxy-url=*)
      val="${1#*=}"
      mcp_args+=("--proxy-url=$(container_proxy_url "$val")")
      exec_mcp_args+=("--proxy-url=$(exec_proxy_url "$val")")
      shift ;;
    *)
      mcp_args+=("$1")
      exec_mcp_args+=("$1")
      shift ;;
  esac
done

# Fast path: reuse the running persistent proxy container instead of spawning
# a disposable container per CLI session. Skip when an explicit image is pinned
# or the caller opts out via HEADROOM_MCP_NO_EXEC=1.
if [[ -z "${HEADROOM_DOCKER_IMAGE:-}" && "${HEADROOM_MCP_NO_EXEC:-0}" != "1" ]]; then
  if [[ "$(docker inspect -f '{{.State.Running}}' "${HEADROOM_CONTAINER_NAME}" 2>/dev/null || true)" == "true" ]]; then
    exec docker exec -i \
      -e "HEADROOM_PROXY_URL=${HEADROOM_EXEC_PROXY_URL_VALUE}" \
      "${HEADROOM_CONTAINER_NAME}" \
      headroom mcp serve ${exec_mcp_args[@]+"${exec_mcp_args[@]}"}
  fi
fi

mkdir -p \
  "${HEADROOM_HOST_HOME}/.headroom/.cache" \
  "${HEADROOM_HOST_HOME}/.claude" \
  "${HEADROOM_HOST_HOME}/.codex" \
  "${HEADROOM_HOST_HOME}/.gemini"

user_args=()
if command -v id >/dev/null 2>&1; then
  user_args=(--user "$(id -u):$(id -g)")
fi

exec docker run --rm --no-healthcheck -i \
  "${user_args[@]}" \
  -w /workspace \
  --env "HOME=${HEADROOM_CONTAINER_HOME}" \
  --env "PYTHONUNBUFFERED=1" \
  --env "XDG_CACHE_HOME=${HEADROOM_CONTAINER_HOME}/.headroom/.cache" \
  --env "HF_HOME=${HEADROOM_CONTAINER_HOME}/.headroom/.cache/huggingface" \
  --env "HEADROOM_WORKSPACE_DIR=${HEADROOM_CONTAINER_HOME}/.headroom" \
  --env "HEADROOM_CONFIG_DIR=${HEADROOM_CONTAINER_HOME}/.headroom/config" \
  --env "HEADROOM_PROXY_URL=${HEADROOM_PROXY_URL_VALUE}" \
  -v "${PWD}:/workspace" \
  -v "${HEADROOM_HOST_HOME}/.headroom:${HEADROOM_CONTAINER_HOME}/.headroom" \
  -v "${HEADROOM_HOST_HOME}/.claude:${HEADROOM_CONTAINER_HOME}/.claude" \
  -v "${HEADROOM_HOST_HOME}/.codex:${HEADROOM_CONTAINER_HOME}/.codex" \
  -v "${HEADROOM_HOST_HOME}/.gemini:${HEADROOM_CONTAINER_HOME}/.gemini" \
  --entrypoint headroom \
  "${HEADROOM_IMAGE}" \
  mcp serve ${mcp_args[@]+"${mcp_args[@]}"}
EOF
  chmod +x "$shim"
  printf '%s\n' "$shim"
}

register_claude_docker_native_mcp() {
  shim="$(write_docker_native_mcp_shim)"
  if command -v claude >/dev/null 2>&1; then
    claude mcp remove headroom -s user >/dev/null 2>&1 || true
    claude mcp add headroom -s user -e "HEADROOM_PROXY_URL=$proxy_url" -- "$shim" >/dev/null || return 1
    log "registered Docker-native Headroom MCP for Claude via $shim"
    return 0
  fi

  mkdir -p "$HOME/.claude"
  config="$HOME/.claude/mcp.json"
  if command -v python3 >/dev/null 2>&1; then
    HEADROOM_SHIM="$shim" HEADROOM_PROXY_URL_VALUE="$proxy_url" HEADROOM_CONFIG="$config" python3 - <<'PY' || return 1
import json, os
from pathlib import Path

path = Path(os.environ["HEADROOM_CONFIG"])
try:
    data = json.loads(path.read_text()) if path.exists() else {}
except Exception:
    data = {}
servers = data.setdefault("mcpServers", {})
servers["headroom"] = {
    "command": os.environ["HEADROOM_SHIM"],
    "args": [],
    "env": {"HEADROOM_PROXY_URL": os.environ["HEADROOM_PROXY_URL_VALUE"]},
}
path.parent.mkdir(parents=True, exist_ok=True)
path.write_text(json.dumps(data, indent=2) + "\n")
PY
    log "registered Docker-native Headroom MCP for Claude in $config"
    return 0
  fi

  warn "Claude CLI not found and python3 unavailable; cannot write Claude MCP config automatically."
  return 1
}

ensure_headroom() {
  if command -v headroom >/dev/null 2>&1; then
    log "headroom already on PATH: $(command -v headroom)"
    if is_headroom_docker_native; then
      headroom_docker_native=1
      log "detected Docker-native Headroom wrapper"
    fi
    headroom --version 2>/dev/null || true
    return 0
  fi

  pybin="$(find_python || true)"
  if [ -z "${pybin:-}" ]; then
    warn "Python 3.10+ not found; trying Docker-native Headroom fallback."
    install_headroom_docker_native
    return $?
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
  if ! "$pybin" -m pip install --user 'headroom-ai[mcp]>=0.25.0'; then
    warn "Python install failed; trying Docker-native Headroom fallback."
    install_headroom_docker_native
    return $?
  fi
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

  if [ "${headroom_docker_native:-0}" = "1" ]; then
    headroom_cmd="$(write_docker_native_mcp_shim)"
  else
    headroom_cmd="$(command -v headroom)"
  fi

  if [ -x "$script_dir/scripts/register_mcp.py" ]; then
    if [ -n "$agent_filter" ]; then
      log "registering Headroom MCP for Makakoo agent '$agent_filter' at $proxy_url"
      python3 "$script_dir/scripts/register_mcp.py" \
        --home "$HOME" \
        --command "$headroom_cmd" \
        --proxy-url "$proxy_url" \
        --agent "$agent_filter" || return 1
    else
      log "registering Headroom MCP for every detected Makakoo CLI at $proxy_url"
      python3 "$script_dir/scripts/register_mcp.py" \
        --home "$HOME" \
        --command "$headroom_cmd" \
        --proxy-url "$proxy_url" || return 1
    fi
  else
    warn "Makakoo Headroom registrar missing at $script_dir/scripts/register_mcp.py; falling back to upstream installer"
    if [ "${headroom_docker_native:-0}" = "1" ]; then
      register_claude_docker_native_mcp || return 1
    elif [ -n "$agent_filter" ]; then
      headroom mcp install --agent "$agent_filter" --proxy-url "$proxy_url" || return 1
    else
      headroom mcp install --proxy-url "$proxy_url" || return 1
    fi
  fi
}

if ensure_headroom; then
  if register_mcp; then
    log "Headroom MCP ready. Restart Claude/Codex/Cursor sessions to pick it up."
  else
    warn "Headroom installed but MCP registration failed. Run: headroom mcp install --proxy-url '$proxy_url'"
  fi
else
  warn "Headroom not installed. Install later with: pipx install --python python3.11 'headroom-ai[mcp]>=0.25.0' || curl -fsSL '$docker_install_url' | bash"
fi

# Install scripts are best-effort; never block a fresh Makakoo OS install.
exit 0

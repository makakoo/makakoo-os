#!/usr/bin/env bash
# install.sh — agent-octopus-peer lifecycle manager.
#
# Manages TWO processes:
#   1. HTTP shim  — Python HTTP front-end on MAKAKOO_MCP_HTTP_PORT (default 8765).
#                   Registered with launchd (macOS) or systemd (Linux) so it
#                   survives terminal exits and restarts on boot.
#   2. harvey-listen.js — Node.js autonomous listener daemon. Started as a
#                   background subprocess managed by this script so
#                   `makakoo agent stop octopus-peer` kills it cleanly.
#
# Usage (direct):
#   install.sh            — install shim launchd/systemd unit (first run)
#   install.sh start      — start shim (launchd/systemd) + listener daemon
#   install.sh stop       — stop listener + unload shim launchd/systemd unit
#   install.sh restart     — stop + start
#   install.sh health     — exit 0 if both shim + listener are healthy
#
# Invoked by AgentRunner as:
#   makakoo agent start octopus-peer   → install.sh start
#   makakoo agent stop  octopus-peer   → install.sh stop
#   makakoo agent health octopus-peer  → install.sh health

set -euo pipefail

PLUGIN_DIR="${MAKAKOO_PLUGIN_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)}"
MAKAKOO_HOME="${MAKAKOO_HOME:-$HOME/MAKAKOO}"
STATE_DIR="${MAKAKOO_HOME}/state/agent-octopus-peer"
LOG_DIR="${STATE_DIR}/logs"
LISTENER_PID_FILE="${STATE_DIR}/listener.pid"
LISTENER_OUT="${LOG_DIR}/listener-stdout.log"
LISTENER_ERR="${LOG_DIR}/listener-stderr.log"

# ── shim env ─────────────────────────────────────────────────────

MAKAKOO_MCP_HTTP_PORT="${MAKAKOO_MCP_HTTP_PORT:-8765}"
MAKAKOO_MCP_HTTP_BIND="${MAKAKOO_MCP_HTTP_BIND:-0.0.0.0}"
SHIM_PYTHONPATH="${MAKAKOO_HOME}/plugins/lib-harvey-core/src"
SHIM_ENTRY="${MAKAKOO_HOME}/plugins/lib-harvey-core/src/core/mcp/http_shim.py"
LISTENER_ENTRY="${MAKAKOO_HOME}/plugins/lib-harvey-core/src/core/harvey-listen.js"

# launchd / systemd IDs
PLIST_NAME="com.makakoo.mcp.http"
PLIST_PATH="${STATE_DIR}/${PLIST_NAME}.plist"
SYSTEMD_NAME="makakoo-mcp-http.service"
SYSTEMD_PATH="${STATE_DIR}/${SYSTEMD_NAME}"

mkdir -p "${STATE_DIR}" "${LOG_DIR}"

# ── platform detection ──────────────────────────────────────────

_is_macos() { [[ "$(uname)" == "Darwin" ]]; }
_is_linux() { [[ "$(uname)" == "Linux" ]]; }

# ── shim helpers ────────────────────────────────────────────────

_shim_running() {
    if _is_macos; then
        launchctl list | grep -q "${PLIST_NAME}" && return 0
    elif _is_linux; then
        systemctl is-active --quiet "${SYSTEMD_NAME}" 2>/dev/null && return 0
    fi
    # Fallback: probe the HTTP port
    curl -sf "http://127.0.0.1:${MAKAKOO_MCP_HTTP_PORT}/rpc" \
        -X POST -H "Content-Type: application/json" \
        -d '{"jsonrpc":"2.0","id":0,"method":"tools/list"}' \
        -m 2 >/dev/null 2>&1 && return 0
    return 1
}

# ── listener helpers ────────────────────────────────────────────

_listener_running() {
    local pid
    pid="$(cat "${LISTENER_PID_FILE}" 2>/dev/null)" || return 1
    kill -0 "${pid}" 2>/dev/null
}

_listener_start() {
    if _listener_running; then
        echo "[agent-octopus-peer] listener already running (pid=$(cat "${LISTENER_PID_FILE}"))"
        return 0
    fi
    if [[ ! -f "${LISTENER_ENTRY}" ]]; then
        echo "ERROR: harvey-listen.js not found at ${LISTENER_ENTRY}"
        echo "  The plugin source must be installed at \$MAKAKOO_HOME/plugins/."
        echo "  Run: makakoo plugin install --core lib-harvey-core"
        exit 1
    fi
    echo "[agent-octopus-peer] starting harvey-listen.js ..."
    # Clean up stale pid file
    rm -f "${LISTENER_PID_FILE}"
    # HARVEY_LISTEN_INTERVAL_S=10 for local testing (override with env)
    # NODE_PATH set so 'require' can find core modules if any are added.
    NODE_PATH="$(dirname "${LISTENER_ENTRY}")" \
        env "HARVEY_LISTEN_INTERVAL_S=${HARVEY_LISTEN_INTERVAL_S:-30}" \
           "HARVEY_LISTEN_NONCE_LRU_SIZE=${HARVEY_LISTEN_NONCE_LRU_SIZE:-100}" \
           "MAKAKOO_HOME=${MAKAKOO_HOME}" \
           node "${LISTENER_ENTRY}" \
        >> "${LISTENER_OUT}" 2>> "${LISTENER_ERR}" &
    local pid=$!
    echo "${pid}" > "${LISTENER_PID_FILE}"
    # Verify it actually started
    sleep 1
    if kill -0 "${pid}" 2>/dev/null; then
        echo "[agent-octopus-peer] listener started (pid=${pid})"
    else
        rm -f "${LISTENER_PID_FILE}"
        echo "ERROR: listener exited immediately. Check ${LISTENER_ERR}."
        exit 1
    fi
}

_listener_stop() {
    local pid
    pid="$(cat "${LISTENER_PID_FILE}" 2>/dev/null)" || {
        echo "[agent-octopus-peer] no listener pid file — nothing to stop"
        return 0
    }
    if kill -0 "${pid}" 2>/dev/null; then
        echo "[agent-octopus-peer] stopping listener (pid=${pid}) ..."
        kill "${pid}"
        # Wait up to 5s for graceful exit
        for i in $(seq 1 10); do
            kill -0 "${pid}" 2>/dev/null || break
            sleep 0.5
        done
        if kill -0 "${pid}" 2>/dev/null; then
            echo "[agent-octopus-peer] listener did not stop gracefully — killing"
            kill -9 "${pid}" 2>/dev/null || true
        fi
        echo "[agent-octopus-peer] listener stopped"
    else
        echo "[agent-octopus-peer] listener not running"
    fi
    rm -f "${LISTENER_PID_FILE}"
}

# ── launchd / systemd units ─────────────────────────────────────

_install_shim() {
    if [[ ! -f "${SHIM_ENTRY}" ]]; then
        echo "ERROR: http_shim.py not found at ${SHIM_ENTRY}"
        echo "  The plugin source must be installed at \$MAKAKOO_HOME/plugins/."
        echo "  Run: makakoo plugin install --core lib-harvey-core"
        exit 1
    fi

    if _is_macos; then
        _install_launchd
    elif _is_linux; then
        _install_systemd
    else
        echo "ERROR: unsupported platform $(uname)"
        exit 1
    fi
}

_install_launchd() {
    cat > "${PLIST_PATH}" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>${PLIST_NAME}</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/bin/python3</string>
        <string>-u</string>
        <string>${SHIM_ENTRY}</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>MAKAKOO_HOME</key><string>${MAKAKOO_HOME}</string>
        <key>PYTHONPATH</key><string>${SHIM_PYTHONPATH}</string>
        <key>MAKAKOO_MCP_HTTP_BIND</key><string>${MAKAKOO_MCP_HTTP_BIND}</string>
        <key>MAKAKOO_MCP_HTTP_PORT</key><string>${MAKAKOO_MCP_HTTP_PORT}</string>
    </dict>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>StandardOutPath</key><string>${LOG_DIR}/shim-stdout.log</string>
    <key>StandardErrorPath</key><string>${LOG_DIR}/shim-stderr.log</string>
    <key>WorkingDirectory</key><string>${STATE_DIR}</string>
</dict>
</plist>
EOF
    chmod 644 "${PLIST_PATH}"
    echo "  launchd plist written to ${PLIST_PATH}"
}

_install_systemd() {
    cat > "${SYSTEMD_PATH}" <<EOF
[Unit]
Description=makakoo-mcp HTTP Shim — signed peer MCP endpoint
After=network.target

[Service]
Type=simple
ExecStart=/usr/bin/python3 -u ${SHIM_ENTRY}
Environment="MAKAKOO_HOME=${MAKAKOO_HOME}"
Environment="PYTHONPATH=${SHIM_PYTHONPATH}"
Environment="MAKAKOO_MCP_HTTP_BIND=${MAKAKOO_MCP_HTTP_BIND}"
Environment="MAKAKOO_MCP_HTTP_PORT=${MAKAKOO_MCP_HTTP_PORT}"
Restart=always
RestartSec=5
WorkingDirectory=${STATE_DIR}
StandardOutput=append:${LOG_DIR}/shim-stdout.log
StandardError=append:${LOG_DIR}/shim-stderr.log

[Install]
WantedBy=multi-user.target
EOF
    chmod 644 "${SYSTEMD_PATH}"
    echo "  systemd unit written to ${SYSTEMD_PATH}"
}

# ── shim lifecycle ───────────────────────────────────────────────

_shim_start() {
    if _shim_running; then
        echo "[agent-octopus-peer] HTTP shim already running"
        return 0
    fi
    if [[ ! -f "${PLIST_PATH}" ]] && [[ ! -f "${SYSTEMD_PATH}" ]]; then
        echo "[agent-octopus-peer] shim unit not installed — running install first"
        _install_shim
    fi

    if _is_macos; then
        if [[ -f "${PLIST_PATH}" ]]; then
            launchctl load "${PLIST_PATH}"
            echo "[agent-octopus-peer] shim started via launchd"
        fi
    elif _is_linux; then
        if [[ -f "${SYSTEMD_PATH}" ]]; then
            sudo cp "${SYSTEMD_PATH}" /etc/systemd/system/ 2>/dev/null || \
                echo "WARNING: could not copy systemd unit — run manually:"
            echo "  sudo cp ${SYSTEMD_PATH} /etc/systemd/system/"
            sudo systemctl daemon-reload 2>/dev/null || true
            sudo systemctl enable --now "${SYSTEMD_NAME}"
            echo "[agent-octopus-peer] shim started via systemd"
        fi
    fi

    # Wait briefly and verify
    sleep 2
    if _shim_running; then
        echo "[agent-octopus-peer] ✓ HTTP shim is healthy on port ${MAKAKOO_MCP_HTTP_PORT}"
    else
        echo "WARNING: shim not responding after 2s. Check ${LOG_DIR}/shim-stderr.log"
    fi
}

_shim_stop() {
    if _is_macos; then
        [[ -f "${PLIST_PATH}" ]] && launchctl unload "${PLIST_PATH}" 2>/dev/null \
            && echo "[agent-octopus-peer] shim unloaded" || echo "[agent-octopus-peer] shim not loaded"
    elif _is_linux; then
        sudo systemctl stop "${SYSTEMD_NAME}" 2>/dev/null \
            && echo "[agent-octopus-peer] shim stopped" || echo "[agent-octopus-peer] shim not running"
    fi
}

# ── unified lifecycle ────────────────────────────────────────────

do_install() {
    echo "[agent-octopus-peer] installing Octopus peer stack ..."
    _install_shim
    echo "✓ agent-octopus-peer installed."
    echo ""
    echo "  Next steps:"
    echo "  1. Configure a peer identity:"
    echo "       mkdir -p \${MAKAKOO_HOME}/config/peers"
    echo "       # Add your Mac's public key to trusted.keys"
    echo "  2. Opt the listener in (pods only):"
    echo "       touch \${MAKAKOO_HOME}/.mcp-keys/listener-enabled"
    echo "  3. Start:"
    echo "       makakoo agent start octopus-peer"
}

do_start() {
    echo "[agent-octopus-peer] starting Octopus peer stack ..."
    _shim_start
    _listener_start
    echo "[agent-octopus-peer] ✓ all components started"
}

do_stop() {
    echo "[agent-octopus-peer] stopping Octopus peer stack ..."
    _listener_stop
    _shim_stop
    echo "[agent-octopus-peer] ✓ stopped"
}

do_restart() {
    do_stop
    sleep 1
    do_start
}

do_health() {
    local ok=0

    if _shim_running; then
        echo "OK  HTTP shim responding on port ${MAKAKOO_MCP_HTTP_PORT}"
    else
        echo "DOWN  HTTP shim not responding"
        ok=1
    fi

    if _listener_running; then
        echo "OK  harvey-listen.js running (pid=$(cat "${LISTENER_PID_FILE}"))"
    else
        echo "DOWN  harvey-listen.js not running"
        ok=1
    fi

    return "${ok}"
}

# ── Main dispatch ────────────────────────────────────────────────

CMD="${1:-}"

case "${CMD}" in
    install|start|stop|restart|health)
        "do_${CMD}"
        ;;
    "")
        echo "Usage: $0 {install|start|stop|restart|health}"
        echo ""
        echo "First run:  $0 install   — write launchd/systemd unit"
        echo "Run:        $0 start     — start shim + listener"
        echo "Stop:       $0 stop     — stop both"
        echo "Check:      $0 health    — exit 0 if healthy"
        exit 0
        ;;
    *)
        echo "Unknown command: ${CMD}"
        echo "Usage: $0 {install|start|stop|restart|health}"
        exit 1
        ;;
esac

#!/usr/bin/env bash
# install.sh — agent-switchailocal
#
# Installs @traylinx/switchailocal npm package, sets up launchd (macOS)
# or systemd (Linux), and manages the service lifecycle.
#
# Usage:
#   install.sh          — install + setup (first run)
#   install.sh start    — start the service
#   install.sh stop     — stop the service
#   install.sh restart  — restart the service
#   install.sh health   — check if service is up

set -euo pipefail

PLUGIN_DIR="${MAKAKOO_PLUGIN_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)}"
STATE_DIR="${MAKAKOO_HOME}/state/agent-switchailocal"
LOG_DIR="${STATE_DIR}/logs"
PLIST_NAME="com.traylinx.switchailocal"
PLIST_PATH="${STATE_DIR}/${PLIST_NAME}.plist"
SYSTEMD_NAME="switchailocal.service"
SYSTEMD_PATH="${STATE_DIR}/${SYSTEMD_NAME}"

mkdir -p "${STATE_DIR}" "${LOG_DIR}"

# ── Helpers ──────────────────────────────────────────────────────

_is_macos() { [[ "$(uname)" == "Darwin" ]]; }
_is_linux() { [[ "$(uname)" == "Linux" ]]; }

_npm_pkg_installed() {
    npm list -g --depth=0 --json 2>/dev/null | \
        grep -q "\"@traylinx/switchailocal\"" || return 1
}

_ail_bin() {
    # The npm package ships a bin.js that downloads the binary on first run.
    # Find it via npm's global bin dir.
    local npm_bin
    npm_bin="$(npm bin -g 2>/dev/null)" && [[ -n "${npm_bin}" ]] && \
        echo "${npm_bin}/switchailocal" && return
    # Fallback: search PATH
    command -v switchailocal 2>/dev/null
}

_switchailocal_running() {
    if _is_macos; then
        launchctl list | grep -q "${PLIST_NAME}" && return 0
    elif _is_linux; then
        systemctl is-active --quiet "${SYSTEMD_NAME}" 2>/dev/null && return 0
    fi
    # Fallback: check if port is listening
    curl -sf "http://127.0.0.1:18080/v1/models" >/dev/null 2>&1 && return 0
    return 1
}

# ── Install ──────────────────────────────────────────────────────

do_install() {
    echo "→ [agent-switchailocal] installing @traylinx/switchailocal..."
    if ! _npm_pkg_installed; then
        npm install -g "@traylinx/switchailocal" --quiet
    else
        echo "  npm package already installed."
    fi

    local ail_bin
    ail_bin="$(_ail_bin)" || { echo "ERROR: switchailocal not found after install"; exit 1; }
    echo "  binary: ${ail_bin}"

    if _is_macos; then
        _install_launchd "${ail_bin}"
    elif _is_linux; then
        _install_systemd "${ail_bin}"
    else
        echo "ERROR: unsupported platform $(uname)"
        exit 1
    fi

    echo "✓ agent-switchailocal installed."
    echo "  Start with: launchctl load ${PLIST_PATH}   (macOS)"
    echo "  Or:         sudo systemctl start ${SYSTEMD_NAME} (Linux)"
}

_install_launchd() {
    local bin="$1"
    cat > "${PLIST_PATH}" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>${PLIST_NAME}</string>
    <key>ProgramArguments</key>
    <array>
        <string>${bin}</string>
        <string>serve</string>
    </array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>StandardOutPath</key><string>${LOG_DIR}/stdout.log</string>
    <key>StandardErrorPath</key><string>${LOG_DIR}/stderr.log</string>
    <key>WorkingDirectory</key><string>${STATE_DIR}</string>
</dict>
</plist>
EOF
    chmod 644 "${PLIST_PATH}"
    echo "  launchd plist written to ${PLIST_PATH}"
}

_install_systemd() {
    local bin="$1"
    cat > "${SYSTEMD_PATH}" <<EOF
[Unit]
Description=switchAILocal — unified local LLM gateway
After=network.target

[Service]
Type=simple
ExecStart=${bin} serve
Restart=always
RestartSec=5
WorkingDirectory=${STATE_DIR}
StandardOutput=append:${LOG_DIR}/stdout.log
StandardError=append:${LOG_DIR}/stderr.log

[Install]
WantedBy=multi-user.target
EOF
    chmod 644 "${SYSTEMD_PATH}"
    echo "  systemd unit written to ${SYSTEMD_PATH}"
    echo "  To install: sudo cp ${SYSTEMD_PATH} /etc/systemd/system/"
    echo "              sudo systemctl daemon-reload"
}

# ── Service lifecycle ─────────────────────────────────────────────

do_start() {
    if _is_macos; then
        if [[ -f "${PLIST_PATH}" ]]; then
            launchctl load "${PLIST_PATH}"
            echo "✓ switchailocal started (launchd)"
        else
            echo "ERROR: plist not found. Run install.sh first."
            exit 1
        fi
    elif _is_linux; then
        if [[ -f "${SYSTEMD_PATH}" ]]; then
            sudo cp "${SYSTEMD_PATH}" /etc/systemd/system/ 2>/dev/null || \
                echo "WARNING: could not copy to /etc/systemd/system/ — run manually:"
            echo "  sudo cp ${SYSTEMD_PATH} /etc/systemd/system/"
            sudo systemctl daemon-reload 2>/dev/null || true
            sudo systemctl enable --now "${SYSTEMD_NAME}"
            echo "✓ switchailocal started (systemd)"
        else
            echo "ERROR: systemd unit not found. Run install.sh first."
            exit 1
        fi
    else
        echo "ERROR: unsupported platform"
        exit 1
    fi
}

do_stop() {
    if _is_macos; then
        launchctl unload "${PLIST_PATH}" 2>/dev/null && echo "✓ stopped" || echo "already stopped"
    elif _is_linux; then
        sudo systemctl stop "${SYSTEMD_NAME}" 2>/dev/null && echo "✓ stopped" || echo "already stopped"
    fi
}

do_restart() {
    do_stop
    sleep 1
    do_start
}

do_health() {
    if _switchailocal_running; then
        echo "OK — switchailocal is running on port 18080"
        return 0
    else
        echo "DOWN — switchailocal is not running"
        return 1
    fi
}

# ── Main ─────────────────────────────────────────────────────────

CMD="${1:-install}"

case "${CMD}" in
    install|start|stop|restart|health)
        "do_${CMD}"
        ;;
    *)
        echo "Usage: $0 {install|start|stop|restart|health}"
        exit 1
        ;;
esac

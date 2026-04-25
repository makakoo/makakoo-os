#!/usr/bin/env bash
# garage-store install hook.
#
# Runs at `makakoo plugin install --core garage-store` time. Idempotent.
# Verifies the host is macOS + has Homebrew, ensures `garage` is on PATH
# (installing via `brew install garage` if missing), and seeds the default
# garage.toml config if absent.
#
# Garage is sourced via Homebrew because upstream ships no native macOS
# binary (P0.1 verification, 2026-04-25). The brew formula compiles from
# the AGPL upstream tarball at git.deuxfleurs.fr/Deuxfleurs/garage v2.3.0
# (sha256 b83a981677676b35400bbbaf20974c396f32da31c7c7630ce55fc3e62c0e2e01).

set -euo pipefail

GARAGE_TARBALL_SHA="b83a981677676b35400bbbaf20974c396f32da31c7c7630ce55fc3e62c0e2e01"

# 1. macOS gate. v0.7 ships macOS-only per Locked Decision 15.
if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "garage-store: refusing install on $(uname -s)." >&2
    echo "Makakoo garage-store v0.7 supports macOS only." >&2
    echo "Linux/Windows support ships in v0.8." >&2
    echo "See: docs/platform-matrix.md" >&2
    exit 1
fi

# 2. Homebrew gate.
if ! command -v brew >/dev/null 2>&1; then
    echo "garage-store: Homebrew not found on PATH." >&2
    echo "Install Homebrew first: https://brew.sh" >&2
    exit 1
fi

# 3. Garage install. Idempotent — `brew install garage` returns 0 on
#    "already installed".
if ! command -v garage >/dev/null 2>&1; then
    echo "garage-store: installing Garage via Homebrew..."
    brew install garage
fi

# 4. Sanity check: `garage --version` matches the pinned formula.
GARAGE_VERSION="$(garage --version 2>&1 | head -n1 || true)"
if [[ -z "$GARAGE_VERSION" ]]; then
    echo "garage-store: \`garage --version\` returned empty output." >&2
    exit 1
fi
echo "garage-store: $GARAGE_VERSION"
echo "garage-store: AGPL pin = $GARAGE_TARBALL_SHA (formula upstream tarball)"

# 5. Seed default config if absent.
MAKAKOO_HOME="${MAKAKOO_HOME:-${HARVEY_HOME:-$HOME/MAKAKOO}}"
CONFIG_PATH="$MAKAKOO_HOME/config/garage.toml"
if [[ ! -f "$CONFIG_PATH" ]]; then
    mkdir -p "$MAKAKOO_HOME/config"
    mkdir -p "$MAKAKOO_HOME/data/garage" "$MAKAKOO_HOME/data/garage-meta"
    # 32 hex chars for rpc_secret, base64 for admin/metrics tokens.
    RPC_SECRET="$(openssl rand -hex 32)"
    ADMIN_TOKEN="$(openssl rand -base64 32 | tr -d '=' | tr '+/' '-_')"
    METRICS_TOKEN="$(openssl rand -base64 32 | tr -d '=' | tr '+/' '-_')"
    cat > "$CONFIG_PATH" <<TOML
metadata_dir = "$MAKAKOO_HOME/data/garage-meta"
data_dir     = "$MAKAKOO_HOME/data/garage"
db_engine    = "sqlite"
replication_factor = 1
rpc_bind_addr = "127.0.0.1:3901"
rpc_public_addr = "127.0.0.1:3901"
rpc_secret    = "$RPC_SECRET"

[s3_api]
s3_region = "garage"
api_bind_addr = "127.0.0.1:3900"
root_domain = ".s3.garage.localhost"

[admin]
api_bind_addr = "127.0.0.1:3903"
admin_token = "$ADMIN_TOKEN"
metrics_token = "$METRICS_TOKEN"
TOML
    chmod 0600 "$CONFIG_PATH"
    echo "garage-store: seeded $CONFIG_PATH"
else
    echo "garage-store: config already present at $CONFIG_PATH"
fi

# 6. Seed launchd plist. The plist owns the process lifecycle —
#    KeepAlive + RunAtLoad means launchd auto-restarts on crash and
#    auto-loads on reboot. Plugin runner only does launchctl load/unload.
PLIST_PATH="$HOME/Library/LaunchAgents/com.makakoo.garage.plist"
GARAGE_BIN="$(command -v garage)"
LOG_DIR="$HOME/Library/Logs/makakoo"
mkdir -p "$LOG_DIR" "$HOME/Library/LaunchAgents"
cat > "$PLIST_PATH" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>com.makakoo.garage</string>
    <key>ProgramArguments</key>
    <array>
      <string>$GARAGE_BIN</string>
      <string>-c</string>
      <string>$CONFIG_PATH</string>
      <string>server</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>$LOG_DIR/garage.out.log</string>
    <key>StandardErrorPath</key>
    <string>$LOG_DIR/garage.err.log</string>
    <key>EnvironmentVariables</key>
    <dict>
      <key>HOME</key>
      <string>$HOME</string>
    </dict>
    <key>ProcessType</key>
    <string>Background</string>
  </dict>
</plist>
PLIST
chmod 0644 "$PLIST_PATH"
echo "garage-store: seeded launchd plist at $PLIST_PATH"

echo "garage-store: install complete."
echo "Next: makakoo plugin start garage-store"

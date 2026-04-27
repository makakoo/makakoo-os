#!/usr/bin/env bash
# Makakoo OS installer — macOS + Linux
#
# Usage:
#   curl -sSL https://makakoo.com/install | sh
#
# Or with flags (download then re-exec):
#   curl -sSL https://makakoo.com/install | sh -s -- --version 0.1.0 --dry-run
#
# After the binary is on disk, finish setup with:
#   makakoo install
#
# This script only places the `makakoo` binary on $PATH. The `makakoo
# install` step (distro + daemon + infect) is a separate command so
# users can review it before running.

set -euo pipefail

# ─── defaults ────────────────────────────────────────────────────────────

VERSION="${MAKAKOO_VERSION:-latest}"
INSTALL_DIR="${MAKAKOO_INSTALL_DIR:-$HOME/.local/bin}"
REPO="${MAKAKOO_REPO:-makakoo/makakoo-os}"
DRY_RUN=0
LOCAL_TARBALL="${MAKAKOO_LOCAL_TARBALL:-}"

# ─── arg parsing ─────────────────────────────────────────────────────────

usage() {
    cat <<EOF
Makakoo OS installer

Usage: install.sh [options]

Options:
  --version VERSION     Install a specific release (default: latest)
  --install-dir DIR     Install binary into DIR (default: \$HOME/.local/bin)
  --dry-run             Print what would happen without downloading
  -h, --help            Show this message

Environment overrides:
  MAKAKOO_VERSION       Same as --version
  MAKAKOO_INSTALL_DIR   Same as --install-dir
  MAKAKOO_REPO          GitHub org/repo (default: makakoo/makakoo-os)
  MAKAKOO_LOCAL_TARBALL Use a local tarball path instead of downloading

After installing, run:
  makakoo install       Install core distro, daemon, infect CLI hosts
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version)       VERSION="$2"; shift 2 ;;
        --install-dir)   INSTALL_DIR="$2"; shift 2 ;;
        --dry-run)       DRY_RUN=1; shift ;;
        -h|--help)       usage; exit 0 ;;
        *)               echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
done

# ─── detect target triple ────────────────────────────────────────────────

uname_s="$(uname -s)"
uname_m="$(uname -m)"

case "$uname_s" in
    Darwin)
        case "$uname_m" in
            arm64|aarch64) TARGET="aarch64-apple-darwin" ;;
            x86_64)        TARGET="x86_64-apple-darwin" ;;
            *)             echo "unsupported arch on macOS: $uname_m" >&2; exit 1 ;;
        esac
        ;;
    Linux)
        case "$uname_m" in
            x86_64)        TARGET="x86_64-unknown-linux-gnu" ;;
            aarch64|arm64) TARGET="aarch64-unknown-linux-gnu" ;;
            *)             echo "unsupported arch on Linux: $uname_m" >&2; exit 1 ;;
        esac
        ;;
    *)
        echo "unsupported OS: $uname_s" >&2
        echo "Windows users: use install.ps1 instead." >&2
        exit 1
        ;;
esac

# ─── resolve URL ─────────────────────────────────────────────────────────

if [ "$VERSION" = "latest" ]; then
    TAR_URL="https://github.com/${REPO}/releases/latest/download/makakoo-${TARGET}.tar.gz"
else
    # Accept both "0.1.0" and "v0.1.0" shapes.
    v="$VERSION"
    case "$v" in v*) ;; *) v="v$v" ;; esac
    TAR_URL="https://github.com/${REPO}/releases/download/${v}/makakoo-${TARGET}.tar.gz"
fi

# ─── print plan ──────────────────────────────────────────────────────────

echo "Makakoo OS installer"
echo "  target:       $TARGET"
echo "  version:      $VERSION"
echo "  install dir:  $INSTALL_DIR"
if [ -n "$LOCAL_TARBALL" ]; then
    echo "  source:       $LOCAL_TARBALL  (local tarball, skipping download)"
else
    echo "  source:       $TAR_URL"
fi

if [ "$DRY_RUN" = "1" ]; then
    echo
    echo "--dry-run: no changes made"
    exit 0
fi

# ─── download + extract ──────────────────────────────────────────────────

tmp="$(mktemp -d)"
cleanup() { rm -rf "$tmp"; }
trap cleanup EXIT INT TERM

tarball="$tmp/makakoo.tar.gz"

if [ -n "$LOCAL_TARBALL" ]; then
    if [ ! -f "$LOCAL_TARBALL" ]; then
        echo "local tarball not found: $LOCAL_TARBALL" >&2
        exit 1
    fi
    cp "$LOCAL_TARBALL" "$tarball"
else
    echo
    echo "downloading…"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$TAR_URL" -o "$tarball"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$tarball" "$TAR_URL"
    else
        echo "neither curl nor wget found on PATH" >&2
        exit 1
    fi
fi

echo "extracting…"
tar -xzf "$tarball" -C "$tmp"

# The release tarball layout produced by cargo-dist puts the binary
# at the archive root. If a future layout nests it under a subdir,
# this findcall still locates it.
BIN_SRC="$(find "$tmp" -maxdepth 2 -type f -name makakoo -perm -u+x 2>/dev/null | head -n1)"
if [ -z "$BIN_SRC" ]; then
    echo "makakoo binary not found in tarball" >&2
    ls -R "$tmp" >&2
    exit 1
fi

# ─── install ─────────────────────────────────────────────────────────────

mkdir -p "$INSTALL_DIR"
BIN_DST="$INSTALL_DIR/makakoo"
mv -f "$BIN_SRC" "$BIN_DST"
chmod +x "$BIN_DST"

echo
echo "installed: $BIN_DST"

# ─── PATH hint ───────────────────────────────────────────────────────────

case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
        echo
        echo "note: $INSTALL_DIR is not on your PATH."
        echo "add this to your shell profile (~/.zshrc, ~/.bashrc, …):"
        echo "    export PATH=\"$INSTALL_DIR:\$PATH\""
        ;;
esac

cat <<EOF

next step:
    $BIN_DST install

This runs the core distro install, registers the daemon agent, and
infects every detected AI CLI host with the Makakoo bootstrap block.
EOF

#!/usr/bin/env bash
# Makakoo OS installer — macOS + Linux
#
# Usage:
#   curl -sSL https://makakoo.com/install | bash
#
# Or with flags (download then re-exec):
#   curl -sSL https://makakoo.com/install | bash -s -- --version 0.1.0 --dry-run
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
  MAKAKOO_BASE_URL      Download origin for tarball + .sha256 (mirror/air-gap)
  MAKAKOO_SKIP_CHECKSUM Set to 1 to skip sha256 verification (unpublished builds)

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

# MAKAKOO_BASE_URL overrides the download origin — useful for an internal
# mirror, an air-gapped relay, or the installer's own test harness. The
# tarball and its `.sha256` sidecar are both fetched from "$MAKAKOO_BASE_URL/"
# and verification still applies.
if [ -n "${MAKAKOO_BASE_URL:-}" ]; then
    TAR_URL="${MAKAKOO_BASE_URL%/}/makakoo-${TARGET}.tar.gz"
elif [ "$VERSION" = "latest" ]; then
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

# sha256 of a file → lowercase hex on stdout; nonzero if no tool available.
_sha256() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | awk '{print $1}'
    elif command -v openssl >/dev/null 2>&1; then openssl dgst -sha256 "$1" | awk '{print $NF}'
    else return 1
    fi
}

# fetch URL → dest with curl then wget; nonzero on failure.
_fetch() {
    if command -v curl >/dev/null 2>&1; then curl -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then wget -qO "$2" "$1"
    else echo "neither curl nor wget found on PATH" >&2; return 2
    fi
}

# Verify $1 (a downloaded file) against the .sha256 sidecar at $2.sha256.
# Fails closed: a missing tool, a missing sidecar, or a mismatch all abort.
verify_checksum() {
    if [ "${MAKAKOO_SKIP_CHECKSUM:-0}" = "1" ]; then
        echo "warning: MAKAKOO_SKIP_CHECKSUM=1 — skipping integrity verification" >&2
        return 0
    fi
    local actual expected
    actual="$(_sha256 "$1")" || {
        echo "error: no sha256 tool (need sha256sum, shasum, or openssl); cannot verify." >&2
        echo "       re-run with MAKAKOO_SKIP_CHECKSUM=1 to install without verification." >&2
        exit 1
    }
    if ! _fetch "$2.sha256" "$1.sha256"; then
        echo "error: could not download checksum $2.sha256" >&2
        echo "       refusing to install unverified bytes (set MAKAKOO_SKIP_CHECKSUM=1 to override)." >&2
        exit 1
    fi
    expected="$(awk '{print tolower($1); exit}' "$1.sha256")"
    actual="$(printf '%s' "$actual" | tr 'A-F' 'a-f')"
    if [ -z "$expected" ] || [ "$actual" != "$expected" ]; then
        echo "error: checksum mismatch — refusing to install." >&2
        echo "  expected: ${expected:-<empty>}" >&2
        echo "  actual:   $actual" >&2
        exit 1
    fi
    echo "✓ sha256 verified"
}

if [ -n "$LOCAL_TARBALL" ]; then
    if [ ! -f "$LOCAL_TARBALL" ]; then
        echo "local tarball not found: $LOCAL_TARBALL" >&2
        exit 1
    fi
    # Local tarballs are user-supplied bytes; no remote sidecar to verify.
    cp "$LOCAL_TARBALL" "$tarball"
else
    echo
    echo "downloading…"
    _fetch "$TAR_URL" "$tarball" || { echo "download failed: $TAR_URL" >&2; exit 1; }
    verify_checksum "$tarball" "$TAR_URL"
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

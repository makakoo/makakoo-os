#!/usr/bin/env bash
set -euo pipefail

# Makakoo OS installer
# Usage: curl -fsSL https://makakoo.com/install.sh | sh
#
# Env overrides:
#   MAKAKOO_VERSION         — version tag to install (default: latest)
#   MAKAKOO_PREFIX          — install prefix (default: $HOME/.local)
#   MAKAKOO_LOCAL_TARBALL   — path to a pre-downloaded .tar.gz; skips
#                             the curl step. Used by the smoke.yml CI
#                             workflow to test the install path against
#                             pre-published candidate artifacts (and
#                             against private-release artifacts that
#                             can't be curled anonymously).

REPO="makakoo/makakoo-os"
VERSION="${MAKAKOO_VERSION:-latest}"
PREFIX="${MAKAKOO_PREFIX:-$HOME/.local}"

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$OS-$ARCH" in
  darwin-arm64|darwin-aarch64)  TARGET="aarch64-apple-darwin" ;;
  darwin-x86_64)                TARGET="x86_64-apple-darwin" ;;
  linux-x86_64)                 TARGET="x86_64-unknown-linux-gnu" ;;
  linux-aarch64|linux-arm64)    TARGET="aarch64-unknown-linux-gnu" ;;
  *)
    echo "error: unsupported platform: $OS-$ARCH" >&2
    echo "supported: darwin-arm64, darwin-x86_64, linux-x86_64, linux-aarch64" >&2
    exit 1
    ;;
esac

echo "→ installing makakoo ($VERSION) for $TARGET into $PREFIX/bin"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# Smoke-test bypass: if a local tarball path is given, copy it into
# place and skip the GitHub download. The rest of the script (untar +
# install + permissions) runs unchanged so the install code path is
# exercised end-to-end.
if [ -n "${MAKAKOO_LOCAL_TARBALL:-}" ]; then
  if [ ! -f "$MAKAKOO_LOCAL_TARBALL" ]; then
    echo "error: MAKAKOO_LOCAL_TARBALL=$MAKAKOO_LOCAL_TARBALL does not exist" >&2
    exit 1
  fi
  echo "→ using local tarball $MAKAKOO_LOCAL_TARBALL (skipping download)"
  cp "$MAKAKOO_LOCAL_TARBALL" "$TMP/makakoo.tar.gz"
else
  if [ "$VERSION" = "latest" ]; then
    URL="https://github.com/$REPO/releases/latest/download/makakoo-$TARGET.tar.gz"
  else
    URL="https://github.com/$REPO/releases/download/v$VERSION/makakoo-$TARGET.tar.gz"
  fi
  echo "→ downloading $URL"
  if ! curl -fsSL "$URL" -o "$TMP/makakoo.tar.gz"; then
    echo "error: download failed — check version tag and network" >&2
    exit 1
  fi
fi

tar -xzf "$TMP/makakoo.tar.gz" -C "$TMP"

mkdir -p "$PREFIX/bin"

# Some release tarballs nest the binaries under a top-level dir; handle both.
if [ -f "$TMP/makakoo" ]; then
  install -m 755 "$TMP/makakoo"     "$PREFIX/bin/makakoo"
  install -m 755 "$TMP/makakoo-mcp" "$PREFIX/bin/makakoo-mcp"
else
  INNER="$(find "$TMP" -type f -name makakoo -not -name '*.tar.gz' | head -n1)"
  INNER_DIR="$(dirname "$INNER")"
  install -m 755 "$INNER_DIR/makakoo"     "$PREFIX/bin/makakoo"
  install -m 755 "$INNER_DIR/makakoo-mcp" "$PREFIX/bin/makakoo-mcp"
fi

echo "✓ installed makakoo to $PREFIX/bin"
echo ""
echo "  run: $PREFIX/bin/makakoo version"
echo "  add to PATH: export PATH=\"\$PATH:$PREFIX/bin\""
echo ""
echo "→ next steps:"
echo "    $PREFIX/bin/makakoo infect --global    # onboard your AI CLIs"
echo "    $PREFIX/bin/makakoo daemon install     # auto-start on login"

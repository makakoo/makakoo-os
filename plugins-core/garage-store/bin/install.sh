#!/usr/bin/env bash
# garage-store install hook (Phase D adapter shim).
#
# As of GARAGETYTUS-V0.1, the daemon lifecycle and Garage acquisition
# move to the standalone `garagetytus` binary. This file used to be
# the install path; now it's a deferral pointer. Old behaviour
# preserved at git history (commit 099daed8 / pre-Phase-D bodies).

set -euo pipefail

if command -v garagetytus >/dev/null 2>&1; then
    echo "garage-store: garagetytus already on PATH ($(garagetytus --version))"
    echo "garage-store: deferring to standalone install — run \`garagetytus install\` if needed"
    exit 0
fi

cat >&2 <<'EOF'
garage-store: garagetytus is the new install path.

  macOS: brew install traylinx/tap/garagetytus
  Linux: curl -fsSL garagetytus.dev/install | sh
  Windows: targets v0.2

After installing garagetytus, run:

  garagetytus install
  garagetytus start
  garagetytus bootstrap

`makakoo bucket *` commands then forward to it transparently
(per Q2 verdict — Makakoo wraps, garagetytus owns the lifecycle).
EOF
exit 1

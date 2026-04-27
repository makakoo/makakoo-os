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
garage-store: plugin installed in degraded state — `garagetytus`
binary is missing. Shared-storage commands will print the install
hint when invoked; everything else in Makakoo continues to work.

To enable the storage commands, install garagetytus:

  macOS: brew install traylinx/tap/garagetytus
  Linux: curl -fsSL garagetytus.dev/install | sh
  Windows: targets v0.2

Then:

  garagetytus install
  garagetytus start
  garagetytus bootstrap

`makakoo bucket *` will forward to it transparently after that
(per Q2 verdict — Makakoo wraps, garagetytus owns the lifecycle).
EOF
# Soft-fail: plugin install completes so the manifest is registered
# in `core` / `sebastian` distros without breaking install. Runtime
# invocations of `makakoo bucket *` still error cleanly with the
# install hint until the binary is present.
exit 0

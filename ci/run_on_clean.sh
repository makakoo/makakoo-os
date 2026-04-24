#!/usr/bin/env bash
# run_on_clean.sh — provision a minimal, ephemeral makakoo install in a
# scratch HOME, then hand off to verify-docs.sh which invokes the Python
# block-runner against the manifest.
#
# The goal is simple: whatever the docs say you can run, a CI runner with
# no prior makakoo state must be able to run the same commands and get the
# expected output (modulo version skew, which the block-runner tolerates).
#
# Designed for GitHub Actions ubuntu-latest + macOS-latest runners. On a
# developer machine, running this WILL mutate $MAKAKOO_HOME pointed at
# a temp dir; your real ~/MAKAKOO is untouched.
#
# Usage:
#   ci/run_on_clean.sh               # full provision + verify
#   ci/run_on_clean.sh --keep-home   # leave $MAKAKOO_HOME for inspection
#   ci/run_on_clean.sh --skip-install # assume makakoo already installed
#                                      # (fast path for local iteration)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

KEEP_HOME=0
SKIP_INSTALL=0
for arg in "$@"; do
    case "$arg" in
        --keep-home)   KEEP_HOME=1 ;;
        --skip-install) SKIP_INSTALL=1 ;;
        *)             echo "unknown flag: $arg" >&2; exit 2 ;;
    esac
done

# Create a clean MAKAKOO_HOME for this run.
SCRATCH_HOME="$(mktemp -d -t makakoo-docs-verify-XXXXXX)"
export MAKAKOO_HOME="$SCRATCH_HOME"
export HARVEY_HOME="$SCRATCH_HOME"   # legacy alias, kept for bootstrap compat
echo "==> scratch MAKAKOO_HOME: $MAKAKOO_HOME"

cleanup() {
    if [[ "$KEEP_HOME" == "0" ]]; then
        rm -rf "$SCRATCH_HOME"
        echo "==> cleaned up $SCRATCH_HOME"
    else
        echo "==> kept $SCRATCH_HOME for inspection"
    fi
}
trap cleanup EXIT

# ───── Provision makakoo if not already on $PATH (or forced fresh) ─────
if [[ "$SKIP_INSTALL" == "0" ]]; then
    if ! command -v makakoo >/dev/null 2>&1; then
        echo "==> makakoo not on PATH; installing from source"
        cargo install --path makakoo --locked
        cargo install --path makakoo-mcp --locked
    else
        echo "==> makakoo already on PATH: $(command -v makakoo)"
    fi
    echo "==> running makakoo install (--yes --skip-daemon --skip-infect --no-setup for CI)"
    makakoo install --yes --skip-daemon --skip-infect --no-setup
fi

# ───── Run the docs verifier ─────
echo "==> running ci/block_runner.py"
python3 "$REPO_ROOT/ci/block_runner.py" --manifest "$REPO_ROOT/ci/docs_manifest.toml"

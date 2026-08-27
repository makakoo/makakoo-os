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

# ───── Provision the current worktree, never an older PATH install ─────
if [[ "$SKIP_INSTALL" == "0" ]]; then
    TOOL_ROOT="$SCRATCH_HOME/toolchain"
    echo "==> installing current worktree binaries under $TOOL_ROOT"
    cargo install --path makakoo --locked --root "$TOOL_ROOT"
    cargo install --path makakoo-mcp --locked --root "$TOOL_ROOT"
    export PATH="$TOOL_ROOT/bin:$PATH"
    echo "==> running makakoo install (--yes --skip-daemon --skip-infect --no-setup for CI)"
    makakoo install --yes --skip-daemon --skip-infect --no-setup
fi

# CI downloads binaries built from the same commit. Local --skip-install
# callers must prepend target/debug (or another current-tree build) to PATH.
EXPECTED_VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)"
ACTUAL_VERSION="$(makakoo --version 2>/dev/null || true)"
if [[ "$ACTUAL_VERSION" != *"$EXPECTED_VERSION"* ]]; then
    echo "error: docs gate requires makakoo $EXPECTED_VERSION from this worktree; found: ${ACTUAL_VERSION:-missing}" >&2
    echo "hint: cargo build --locked -p makakoo -p makakoo-mcp && PATH=\"$REPO_ROOT/target/debug:\$PATH\" ci/run_on_clean.sh --skip-install" >&2
    exit 2
fi

# ───── Run the docs verifier ─────
echo "==> running ci/block_runner.py"
python3 "$REPO_ROOT/ci/block_runner.py" --manifest "$REPO_ROOT/ci/docs_manifest.toml"

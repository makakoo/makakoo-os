#!/usr/bin/env bash
# verify-docs.sh — top-level harness entrypoint.
#
# In CI (GitHub Actions), the workflow at .github/workflows/verify-docs.yml
# calls this script after checkout + a fast `cargo install --path makakoo`.
# Locally, developers can run it directly; it will use the manifest to pick
# files to verify.
#
# Exit codes:
#   0 — every block passed or was skipped with a reason
#   1 — at least one block failed (output names the file + block index)
#   2 — usage / config error

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

MANIFEST="$REPO_ROOT/ci/docs_manifest.toml"

if [[ ! -f "$MANIFEST" ]]; then
    echo "error: manifest not found at $MANIFEST" >&2
    exit 2
fi

echo "==> verify-docs.sh starting"
echo "    manifest: $MANIFEST"
echo "    python:   $(command -v python3)"

exec python3 "$REPO_ROOT/ci/block_runner.py" --manifest "$MANIFEST" "$@"

#!/usr/bin/env bash
# test_harness.sh — self-test for the docs block-runner itself.
#
# Creates synthetic Markdown files exercising known-good, known-bad, and
# skip-with-reason scenarios, runs the block-runner against them, and
# asserts the exit code + output match expectations.
#
# Run locally before modifying ci/block_runner.py. Wired into CI as a
# sanity-check step that runs BEFORE the real manifest verification.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

PY="python3 $REPO_ROOT/ci/block_runner.py"

TMPDIR="$(mktemp -d -t makakoo-docs-selftest-XXXXXX)"
trap 'rm -rf "$TMPDIR"' EXIT

echo "==> test 1: built-in --self-test"
$PY --self-test

echo "==> test 2: known-good block (pass)"
cat > "$TMPDIR/good.md" <<'EOF'
# good
```sh
echo "hello world"
```
```output
hello world
```
EOF
$PY "$TMPDIR/good.md" >/dev/null
echo "    PASS"

echo "==> test 3: known-bad block (exit 3 should FAIL)"
cat > "$TMPDIR/bad.md" <<'EOF'
# bad
```sh
exit 3
```
EOF
set +e
$PY "$TMPDIR/bad.md" >/dev/null 2>&1
rc=$?
set -e
if [[ "$rc" != "1" ]]; then
    echo "    FAIL — expected exit 1, got $rc" >&2
    exit 1
fi
echo "    PASS"

echo "==> test 4: skip with reason is respected"
cat > "$TMPDIR/skip_ok.md" <<'EOF'
# skip-ok
```sh
rm -rf /
```
<!-- verify: skip reason="destructive example, covered by type system" -->
EOF
$PY "$TMPDIR/skip_ok.md" >/dev/null
echo "    PASS"

echo "==> test 5: skip WITHOUT reason is an error"
cat > "$TMPDIR/skip_bad.md" <<'EOF'
# skip-no-reason
```sh
rm -rf /
```
<!-- verify: skip -->
EOF
set +e
$PY "$TMPDIR/skip_bad.md" >/dev/null 2>&1
rc=$?
set -e
if [[ "$rc" != "1" ]]; then
    echo "    FAIL — expected exit 1, got $rc" >&2
    exit 1
fi
echo "    PASS"

echo "==> test 6: expected-output mismatch is a FAIL"
cat > "$TMPDIR/mismatch.md" <<'EOF'
# mismatch
```sh
echo "actual text"
```
```output
totally different text
```
EOF
set +e
$PY "$TMPDIR/mismatch.md" >/dev/null 2>&1
rc=$?
set -e
if [[ "$rc" != "1" ]]; then
    echo "    FAIL — expected exit 1, got $rc" >&2
    exit 1
fi
echo "    PASS"

echo "==> test 7: loose-match expected output (extra lines OK)"
cat > "$TMPDIR/loose.md" <<'EOF'
# loose
```sh
echo "line1"
echo "noise"
echo "line2"
```
```output
line1
line2
```
EOF
$PY "$TMPDIR/loose.md" >/dev/null
echo "    PASS"

echo
echo "All 7 harness tests passed."

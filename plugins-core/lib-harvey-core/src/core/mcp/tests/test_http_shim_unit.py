#!/usr/bin/env python3
"""
Fast unit tests for http_shim.py internals (no live shim, no subprocess).

Covers:
  - flock interlock: 5 concurrent processes each appending 100 lines to the
    same Brain journal produces a clean 500-line file with zero interleaved
    or truncated entries.
  - nonce extraction symmetry: `extract_nonce` matches every id produced
    by `_nonce_suffix`.

Run directly:
    python3 -m core.mcp.tests.test_http_shim_unit
Or under pytest:
    pytest plugins-core/lib-harvey-core/src/core/mcp/tests/test_http_shim_unit.py
"""

from __future__ import annotations

import multiprocessing as mp
import os
import sys
import tempfile
import unittest
from pathlib import Path

# Make plugins-core/lib-harvey-core/src importable as a package root so
# `core.mcp.http_shim` resolves during the test without requiring an
# installed package.
HERE = Path(__file__).resolve()
SRC_ROOT = HERE.parents[4] / "src"
sys.path.insert(0, str(SRC_ROOT))


class NonceRoundtripTest(unittest.TestCase):
    def test_suffix_then_extract(self):
        from core.mcp.http_shim import _nonce_suffix
        from core.brain_tail import extract_nonce

        samples = [
            "abc123",
            "b7-42-ffff",
            "A1B2C3D4E5F6",
            "e" * 32,  # realistic UUID-ish length
        ]
        for s in samples:
            line = "- some content" + _nonce_suffix(s) + "\n"
            self.assertEqual(extract_nonce(line.rstrip("\n")), s, repr(line))

    def test_none_nonce_produces_no_suffix(self):
        from core.mcp.http_shim import _nonce_suffix
        self.assertEqual(_nonce_suffix(None), "")
        self.assertEqual(_nonce_suffix(""), "")

    def test_extract_nonce_handles_no_token(self):
        from core.brain_tail import extract_nonce
        self.assertIsNone(extract_nonce("- just a human-written line"))
        self.assertIsNone(extract_nonce("- {nonce=} trailing empty"))  # no id


# ────────────────────────── flock concurrency ────────────────────────

def _worker_append(args):
    """Worker run in a child process. Appends N lines to the journal.

    Uses the same `_write_journal_line` primitive the shim exposes, so we
    exercise the real flock path rather than a test-local copy.
    """
    makakoo_home, worker_id, n = args
    os.environ["MAKAKOO_HOME"] = makakoo_home
    # Late import — so the child picks up the env-var we just set before
    # loading the module (MAKAKOO_HOME is read at import time in http_shim).
    sys.path.insert(0, str(SRC_ROOT))
    from core.mcp import http_shim  # noqa: E402

    # Force the journal to live inside our tmp_home.
    http_shim._BRAIN_FLOCK_DIR = os.path.join(makakoo_home, "state", "octopus")
    http_shim._BRAIN_FLOCK_PATH = os.path.join(http_shim._BRAIN_FLOCK_DIR, "brain-write.lock")

    for i in range(n):
        # Use a distinct nonce per line so we can verify post-hoc that
        # (a) every write landed and (b) no partial / truncated line
        # ended up on disk.
        line = f"- worker {worker_id} line {i}"
        http_shim._write_journal_line(line, f"w{worker_id}-{i}")


class FlockConcurrencyTest(unittest.TestCase):
    def test_five_processes_append_500_clean_lines(self):
        with tempfile.TemporaryDirectory() as tmp:
            makakoo_home = os.path.join(tmp, "MAKAKOO")
            os.makedirs(os.path.join(makakoo_home, "data", "Brain", "journals"))
            os.environ["MAKAKOO_HOME"] = makakoo_home

            N_WORKERS = 5
            N_PER_WORKER = 100

            # `spawn` gives us a fresh interpreter per child, matching the
            # real multi-process shape of 5 SME peer-listener processes
            # sharing an NFS mount. Avoids fork-specific quirks.
            ctx = mp.get_context("spawn")
            with ctx.Pool(N_WORKERS) as pool:
                pool.map(
                    _worker_append,
                    [(makakoo_home, w, N_PER_WORKER) for w in range(N_WORKERS)],
                )

            # Locate today's journal. http_shim writes to
            # $MAKAKOO_HOME/data/Brain/journals/YYYY_MM_DD.md — pick it up
            # dynamically so the test doesn't care about the date.
            journals_dir = os.path.join(makakoo_home, "data", "Brain", "journals")
            files = [f for f in os.listdir(journals_dir) if f.endswith(".md")]
            self.assertEqual(len(files), 1, f"expected exactly one journal file, got {files}")
            journal_path = os.path.join(journals_dir, files[0])

            with open(journal_path) as f:
                lines = f.readlines()

            self.assertEqual(
                len(lines), N_WORKERS * N_PER_WORKER,
                f"expected {N_WORKERS * N_PER_WORKER} lines, got {len(lines)}",
            )

            # Every line must be a complete `- worker N line M {nonce=wN-M}\n`
            # with no interleaving (no line contains the marker of a
            # different worker mid-string).
            for line in lines:
                self.assertTrue(line.endswith("\n"), f"truncated line: {line!r}")
                self.assertTrue(line.startswith("- worker "), f"malformed: {line!r}")
                self.assertIn("{nonce=w", line)

            # Every nonce must be unique (proof nothing was double-written
            # or silently dropped).
            from core.brain_tail import extract_nonce  # after sys.path set
            nonces = [extract_nonce(ln.rstrip("\n")) for ln in lines]
            self.assertEqual(
                len(set(nonces)), len(nonces),
                "duplicate or missing nonces indicate interleaved writes",
            )


if __name__ == "__main__":
    unittest.main(verbosity=2)

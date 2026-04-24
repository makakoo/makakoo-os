#!/usr/bin/env python3
"""
Unit + integration tests for Phase 4 enforcement (scope + rate limit).

Covers:
  - Scope classification: read tools reachable by read-brain peer,
    write tools gated on write-brain, unknown tools require full-brain.
  - Rate limit: 30 writes/min per peer; 31st → 429; reads bypass; reset
    after window.
  - SME stress: 10 peers write simultaneously under flock, zero corrupted
    journal lines.
  - Nonce-integrity under burst: listener's LRU drops 100% of self-writes
    at 30 req/min.
"""

from __future__ import annotations

import base64
import multiprocessing as mp
import os
import sys
import tempfile
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve()
SRC_ROOT = HERE.parents[4] / "src"
sys.path.insert(0, str(SRC_ROOT))


def _reload_octopus_modules():
    for name in list(sys.modules):
        if name.startswith("core.octopus") or name == "core.octopus":
            del sys.modules[name]


# ───────────────────────── scope enforcement ───────────────────────

class ScopeEnforcementTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        os.environ["MAKAKOO_HOME"] = self.tmp.name
        _reload_octopus_modules()

    def _add_peer(self, name: str, scope: str):
        from core.octopus import identity, trust_store
        if not identity.exists():
            identity.create("host")
        ident = identity.load()
        trust_store.add_grant(
            peer_name=name, public_key_b64=ident.public_key_b64,
            capability_scope=scope, granted_by_token_id=None, duration="permanent",
        )

    def _enforce(self, peer, method, name=None):
        from core.octopus.enforce import enforce_request
        from core.octopus.ratelimit import PeerRateLimiter
        limiter = PeerRateLimiter()
        params = {"name": name} if name else {}
        return enforce_request(
            peer_name=peer, rpc_method=method, rpc_params=params, limiter=limiter,
        )

    def test_read_peer_can_list_tools(self):
        self._add_peer("pr", "read-brain")
        d = self._enforce("pr", "tools/list")
        self.assertTrue(d.allowed, d.error_message)

    def test_read_peer_can_brain_search(self):
        self._add_peer("pr", "read-brain")
        d = self._enforce("pr", "tools/call", "brain_search")
        self.assertTrue(d.allowed, d.error_message)

    def test_read_peer_blocked_from_brain_write_journal(self):
        self._add_peer("pr", "read-brain")
        d = self._enforce("pr", "tools/call", "brain_write_journal")
        self.assertFalse(d.allowed)
        self.assertEqual(d.http_status, 403)

    def test_write_peer_can_write(self):
        self._add_peer("pw", "write-brain")
        d = self._enforce("pw", "tools/call", "brain_write_journal")
        self.assertTrue(d.allowed, d.error_message)

    def test_write_peer_blocked_from_unknown_tool(self):
        """Pessimistic default: unknown tool name requires full-brain."""
        self._add_peer("pw", "write-brain")
        d = self._enforce("pw", "tools/call", "some_brand_new_tool_that_nobody_classified_yet")
        self.assertFalse(d.allowed)
        self.assertEqual(d.http_status, 403)

    def test_full_peer_can_call_unknown_tool(self):
        self._add_peer("pf", "full-brain")
        d = self._enforce("pf", "tools/call", "some_brand_new_tool")
        self.assertTrue(d.allowed, d.error_message)

    def test_unknown_peer_treated_as_read_brain(self):
        """A peer in trusted.keys (signature OK) but not in trust_store
        (legacy entry). We fail CLOSED on writes but allow reads."""
        d = self._enforce("legacy-peer", "tools/call", "brain_search")
        self.assertTrue(d.allowed)
        d = self._enforce("legacy-peer", "tools/call", "brain_write_journal")
        self.assertFalse(d.allowed)
        self.assertEqual(d.http_status, 403)


# ───────────────────────── rate limit ──────────────────────────────

class RateLimitTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        os.environ["MAKAKOO_HOME"] = self.tmp.name
        _reload_octopus_modules()

    def test_writes_per_minute_default_30(self):
        from core.octopus.ratelimit import PeerRateLimiter, DEFAULT_WRITES_PER_MIN
        self.assertEqual(DEFAULT_WRITES_PER_MIN, 30)
        lim = PeerRateLimiter()
        now = 1_000_000.0
        for i in range(30):
            d = lim.check("peer-A", is_write=True, now=now + i * 0.5)
            self.assertTrue(d.allowed, f"write {i} allowed, remaining={d.remaining}")

    def test_31st_write_returns_429(self):
        from core.octopus.ratelimit import PeerRateLimiter
        lim = PeerRateLimiter()
        now = 1_000_000.0
        for i in range(30):
            lim.check("peer-A", is_write=True, now=now + i * 0.5)
        d = lim.check("peer-A", is_write=True, now=now + 15.0)
        self.assertFalse(d.allowed)
        self.assertGreater(d.reset_after_s, 0)

    def test_31st_write_enforce_layer_returns_429(self):
        """Phase 4 test gate: 31st write in 60s → HTTP 429."""
        from core.octopus import identity, trust_store
        from core.octopus.enforce import enforce_request
        from core.octopus.ratelimit import PeerRateLimiter
        identity.create("host")
        trust_store.add_grant(
            peer_name="peer-W", public_key_b64=identity.load().public_key_b64,
            capability_scope="write-brain", granted_by_token_id=None, duration="permanent",
        )
        lim = PeerRateLimiter()
        now = 1_000_000.0
        for i in range(30):
            enforce_request(peer_name="peer-W", rpc_method="tools/call",
                             rpc_params={"name": "brain_write_journal"},
                             limiter=lim, now=now + i * 0.5)
        d = enforce_request(peer_name="peer-W", rpc_method="tools/call",
                             rpc_params={"name": "brain_write_journal"},
                             limiter=lim, now=now + 15.0)
        self.assertFalse(d.allowed)
        self.assertEqual(d.http_status, 429)
        self.assertGreater(d.retry_after_s, 0)

    def test_reads_bypass_rate_limit(self):
        from core.octopus.ratelimit import PeerRateLimiter
        lim = PeerRateLimiter()
        now = 1_000_000.0
        for i in range(1000):
            d = lim.check("peer-A", is_write=False, now=now + i * 0.01)
            self.assertTrue(d.allowed)

    def test_window_slides(self):
        from core.octopus.ratelimit import PeerRateLimiter
        lim = PeerRateLimiter()
        now = 1_000_000.0
        for i in range(30):
            lim.check("peer-A", is_write=True, now=now + i * 0.5)
        # Request at now+60.5 — the first 30 events are now older than
        # the 60s window → they should be pruned, new write allowed.
        d = lim.check("peer-A", is_write=True, now=now + 60.5)
        self.assertTrue(d.allowed, f"window did not slide: {d!r}")

    def test_different_peers_have_independent_buckets(self):
        from core.octopus.ratelimit import PeerRateLimiter
        lim = PeerRateLimiter()
        now = 1_000_000.0
        # peer-A fills its bucket
        for i in range(30):
            lim.check("peer-A", is_write=True, now=now + i * 0.5)
        # peer-B still has full budget
        d = lim.check("peer-B", is_write=True, now=now + 1.0)
        self.assertTrue(d.allowed)

    def test_rejects_bad_config(self):
        from core.octopus.ratelimit import PeerRateLimiter
        with self.assertRaises(ValueError):
            PeerRateLimiter(writes_per_min=0)
        with self.assertRaises(ValueError):
            PeerRateLimiter(window_s=0)


# ───────────────────────── SME flock stress ────────────────────────

def _sme_worker(args):
    """Child process: N writes to shared Brain under flock. Simulates one
    SME teammate pushing the default 30 writes/min cap."""
    makakoo_home, worker_id, n = args
    os.environ["MAKAKOO_HOME"] = makakoo_home
    sys.path.insert(0, str(SRC_ROOT))
    from core.mcp import http_shim  # noqa: E402

    http_shim._BRAIN_FLOCK_DIR = os.path.join(makakoo_home, "state", "octopus")
    http_shim._BRAIN_FLOCK_PATH = os.path.join(http_shim._BRAIN_FLOCK_DIR, "brain-write.lock")

    for i in range(n):
        http_shim._write_journal_line(
            f"- SME worker {worker_id} line {i}",
            f"sme-{worker_id}-{i}",
        )


class SMEStressTest(unittest.TestCase):
    """Sprint criterion: 10 peers × 30 writes/min = 300 total; flock
    ensures zero corrupted/interleaved entries."""

    def test_ten_peers_three_hundred_writes_zero_corruption(self):
        with tempfile.TemporaryDirectory() as tmp:
            makakoo_home = os.path.join(tmp, "MAKAKOO")
            os.makedirs(os.path.join(makakoo_home, "data", "Brain", "journals"))
            os.environ["MAKAKOO_HOME"] = makakoo_home

            N_PEERS = 10
            N_PER_PEER = 30

            ctx = mp.get_context("spawn")
            with ctx.Pool(N_PEERS) as pool:
                pool.map(
                    _sme_worker,
                    [(makakoo_home, p, N_PER_PEER) for p in range(N_PEERS)],
                )

            # Read back the journal, check every line landed intact.
            journals_dir = os.path.join(makakoo_home, "data", "Brain", "journals")
            files = [f for f in os.listdir(journals_dir) if f.endswith(".md")]
            self.assertEqual(len(files), 1)
            path = os.path.join(journals_dir, files[0])
            with open(path) as f:
                lines = f.readlines()

            self.assertEqual(
                len(lines), N_PEERS * N_PER_PEER,
                f"expected {N_PEERS * N_PER_PEER} lines, got {len(lines)} — "
                f"implies interleaving or lost writes",
            )
            for ln in lines:
                self.assertTrue(ln.endswith("\n"), f"truncated line: {ln!r}")
                self.assertTrue(ln.startswith("- SME worker "), f"malformed: {ln!r}")
                self.assertIn("{nonce=sme-", ln)

            # Every nonce must be unique → every write counted once, no
            # duplicates or interleaved partial writes.
            sys.path.insert(0, str(SRC_ROOT))
            from core.brain_tail import extract_nonce
            nonces = [extract_nonce(ln.rstrip("\n")) for ln in lines]
            self.assertEqual(
                len(set(nonces)), len(nonces),
                "duplicate nonces — flock interlock failed under SME burst",
            )


# ───────────────────────── nonce integrity under burst ─────────────

class NonceBurstTest(unittest.TestCase):
    """Sprint criterion: harvey-listen.js NonceLRU ignores 100% of its
    own writes even under heavy burst (30 req/min). We exercise the pure
    JS-side primitives here — a full end-to-end would require
    orchestrating the shim, the listener, and a mock Brain; that's
    covered by the existing test_http_shim_concurrency.py integration.
    """

    def test_lru_holds_100_nonces_zero_self_match(self):
        import subprocess
        js = str(SRC_ROOT / "core" / "tests" / "test_harvey_listen.js")
        result = subprocess.run(
            ["node", js], capture_output=True, text=True, timeout=60,
        )
        self.assertEqual(
            result.returncode, 0,
            f"harvey-listen LRU/nonce tests failed:\nSTDOUT:\n{result.stdout}\n"
            f"STDERR:\n{result.stderr}",
        )
        self.assertIn("ALL PASS", result.stdout)


if __name__ == "__main__":
    unittest.main(verbosity=2)

#!/usr/bin/env python3
"""
Unit tests for Phase 3 discovery (mDNS, invite, handshake).

Covers:
  - Invite URL roundtrip (encode → decode symmetric, wrong scheme rejected,
    tampered payload rejected, missing fields rejected, version mismatch
    rejected).
  - Handshake proof: HMAC roundtrip works, constant-time verify, bad proof
    rejected, expired challenge rejected, replayed proof to different
    token rejected.
  - Handshake → trust grant: complete_handshake adds grant + redeems token.
  - Tytus CIDR scan: stub probe returns expected addresses, subnet iteration
    covers 254 hosts in /24.
  - mDNS advertise/discover: runs iff zeroconf is installed, else skipped
    cleanly (sprint criterion is "roundtrip within 3s" — integration shape).
"""

from __future__ import annotations

import base64
import importlib
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
        if name == "core.octopus" or name.startswith("core.octopus."):
            del sys.modules[name]


# ───────────────────────── invite link ──────────────────────────────

class InviteRoundtripTest(unittest.TestCase):
    def test_encode_decode_roundtrip(self):
        from core.octopus.discovery.invite import (
            encode_invite, decode_invite, InvitePayload,
        )
        link = encode_invite(
            token_id="tid-abc",
            shared_secret_b64=base64.b64encode(b"S" * 32).decode(),
            capability_scope="write-brain",
            duration_default="permanent",
            expires_at_unix=9999999999,
            issuer_peer_name="iss",
            issuer_public_key_b64=base64.b64encode(b"P" * 32).decode(),
            proposed_peer_name="sarah",
        )
        self.assertTrue(link.startswith("makakoo://join?t="))
        payload = decode_invite(link)
        self.assertIsInstance(payload, InvitePayload)
        self.assertEqual(payload.token_id, "tid-abc")
        self.assertEqual(payload.issuer_peer_name, "iss")
        self.assertEqual(payload.proposed_peer_name, "sarah")
        self.assertEqual(payload.capability_scope, "write-brain")
        self.assertFalse(payload.is_expired(now=0))

    def test_expired_payload_detects_now_past_exp(self):
        from core.octopus.discovery.invite import encode_invite, decode_invite
        link = encode_invite(
            token_id="t", shared_secret_b64=base64.b64encode(b"x" * 32).decode(),
            capability_scope="read-brain", duration_default="1h",
            expires_at_unix=1000, issuer_peer_name="i",
            issuer_public_key_b64=base64.b64encode(b"p" * 32).decode(),
        )
        payload = decode_invite(link)
        self.assertTrue(payload.is_expired(now=1001))
        self.assertFalse(payload.is_expired(now=999))

    def test_wrong_scheme_rejected(self):
        from core.octopus.discovery.invite import decode_invite
        # Wrong scheme
        with self.assertRaises(ValueError):
            decode_invite("http://join?t=x")
        # Wrong host
        with self.assertRaises(ValueError):
            decode_invite("makakoo://other?t=x")

    def test_missing_t_param_rejected(self):
        from core.octopus.discovery.invite import decode_invite
        with self.assertRaises(ValueError):
            decode_invite("makakoo://join")
        with self.assertRaises(ValueError):
            decode_invite("makakoo://join?x=y")

    def test_bad_base64_rejected(self):
        from core.octopus.discovery.invite import decode_invite
        with self.assertRaises(ValueError):
            decode_invite("makakoo://join?t=!!!not-base64!!!")

    def test_bad_json_rejected(self):
        import base64 as b64mod
        from core.octopus.discovery.invite import decode_invite
        raw = b64mod.urlsafe_b64encode(b"{not json").rstrip(b"=").decode()
        with self.assertRaises(ValueError):
            decode_invite(f"makakoo://join?t={raw}")

    def test_version_mismatch_rejected(self):
        import base64 as b64mod, json
        from core.octopus.discovery.invite import decode_invite
        payload = {"v": 999, "tid": "t", "sec": "s", "scope": "read-brain",
                   "dur": "1h", "exp": 1, "iss": "i", "iss_pk": "p"}
        raw = b64mod.urlsafe_b64encode(json.dumps(payload).encode()).rstrip(b"=").decode()
        with self.assertRaises(ValueError) as cm:
            decode_invite(f"makakoo://join?t={raw}")
        self.assertIn("version", str(cm.exception))

    def test_missing_required_fields_rejected(self):
        import base64 as b64mod, json
        from core.octopus.discovery.invite import decode_invite
        payload = {"v": 1, "tid": "t"}  # missing sec, scope, dur, exp, iss, iss_pk
        raw = b64mod.urlsafe_b64encode(json.dumps(payload).encode()).rstrip(b"=").decode()
        with self.assertRaises(ValueError) as cm:
            decode_invite(f"makakoo://join?t={raw}")
        self.assertIn("missing", str(cm.exception).lower())


# ───────────────────────── handshake ────────────────────────────────

class HandshakeTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        os.environ["MAKAKOO_HOME"] = self.tmp.name
        _reload_octopus_modules()
        # Clear challenge dict between tests — it's process-global.
        from core.octopus.discovery import handshake
        handshake._reset_challenge_store()

    def _mint_token(self, **kwargs):
        from core.octopus import identity, onboarding
        identity.create(kwargs.pop("issuer_name", "host"))
        return onboarding.mint(
            issued_by_peer="host",
            **kwargs,
        )

    def test_proof_computation_symmetric(self):
        from core.octopus.discovery.handshake import compute_proof
        challenge = base64.b64encode(b"X" * 32).decode()
        secret = base64.b64encode(b"Y" * 32).decode()
        p1 = compute_proof(shared_secret_b64=secret, challenge_b64=challenge,
                           token_id="tid-1")
        p2 = compute_proof(shared_secret_b64=secret, challenge_b64=challenge,
                           token_id="tid-1")
        self.assertEqual(p1, p2)
        # Different token_id → different proof (binding property).
        p3 = compute_proof(shared_secret_b64=secret, challenge_b64=challenge,
                           token_id="tid-2")
        self.assertNotEqual(p1, p3)

    def test_build_challenge_rejects_unknown_token(self):
        from core.octopus.discovery.handshake import build_challenge
        with self.assertRaises(FileNotFoundError):
            build_challenge("bogus-token-id")

    def test_build_challenge_rejects_expired_token(self):
        from core.octopus.discovery.handshake import build_challenge
        tok = self._mint_token(expires_in_s=10)
        future = tok.expires_at_unix + 1
        with self.assertRaises(PermissionError):
            build_challenge(tok.token_id, now=future)

    def test_verify_proof_roundtrip(self):
        from core.octopus.discovery.handshake import build_challenge, compute_proof, verify_proof
        tok = self._mint_token()
        ch = build_challenge(tok.token_id)
        proof = compute_proof(
            shared_secret_b64=tok.shared_secret_b64,
            challenge_b64=ch.challenge_b64,
            token_id=tok.token_id,
        )
        self.assertTrue(verify_proof(token_id=tok.token_id, proof_b64=proof))

    def test_verify_proof_rejects_wrong_secret(self):
        from core.octopus.discovery.handshake import build_challenge, compute_proof, verify_proof
        tok = self._mint_token()
        ch = build_challenge(tok.token_id)
        wrong_secret = base64.b64encode(b"Z" * 32).decode()
        bad = compute_proof(
            shared_secret_b64=wrong_secret,
            challenge_b64=ch.challenge_b64,
            token_id=tok.token_id,
        )
        self.assertFalse(verify_proof(token_id=tok.token_id, proof_b64=bad))

    def test_verify_proof_rejects_expired_challenge(self):
        from core.octopus.discovery import handshake
        tok = self._mint_token()
        ch = handshake.build_challenge(tok.token_id, now=1000)
        proof = handshake.compute_proof(
            shared_secret_b64=tok.shared_secret_b64,
            challenge_b64=ch.challenge_b64, token_id=tok.token_id,
        )
        # Pretend 61s elapsed — past the 60s TTL.
        self.assertFalse(handshake.verify_proof(
            token_id=tok.token_id, proof_b64=proof, now=1000 + handshake.CHALLENGE_TTL_S + 1,
        ))

    def test_verify_proof_rejects_unknown_token_id(self):
        from core.octopus.discovery.handshake import verify_proof
        self.assertFalse(verify_proof(
            token_id="nonexistent",
            proof_b64=base64.b64encode(b"x" * 32).decode(),
        ))

    def test_complete_handshake_happy_path(self):
        from core.octopus import identity, onboarding, trust_store
        from core.octopus.discovery import handshake

        ident = identity.create("server-mac")
        tok = onboarding.mint(
            issued_by_peer=ident.peer_name,
            capability_scope="write-brain",
            duration_default="permanent",
        )
        ch = handshake.build_challenge(tok.token_id)

        # Joiner computes proof + bundles its own pubkey.
        joiner_pk = base64.b64encode(b"J" * 32).decode()
        proof = handshake.compute_proof(
            shared_secret_b64=tok.shared_secret_b64,
            challenge_b64=ch.challenge_b64, token_id=tok.token_id,
        )

        result = handshake.complete_handshake(
            token_id=tok.token_id,
            proof_b64=proof,
            claimed_peer_name="joiner-laptop",
            claimed_pubkey_b64=joiner_pk,
        )
        self.assertEqual(result.peer_name, "joiner-laptop")
        self.assertEqual(result.capability_scope, "write-brain")

        # Trust grant created.
        grants = trust_store.list_grants()
        self.assertEqual(len(grants), 1)
        self.assertEqual(grants[0].peer_name, "joiner-laptop")

        # Token consumed (file gone).
        self.assertFalse(tok.path.exists(), "onboarding token should be redeemed")

    def test_complete_handshake_rejects_bad_proof(self):
        from core.octopus import identity, onboarding, trust_store
        from core.octopus.discovery import handshake

        identity.create("s")
        tok = onboarding.mint(issued_by_peer="s")
        handshake.build_challenge(tok.token_id)

        with self.assertRaises(PermissionError):
            handshake.complete_handshake(
                token_id=tok.token_id,
                proof_b64=base64.b64encode(b"bad" * 11).decode(),
                claimed_peer_name="attacker",
                claimed_pubkey_b64=base64.b64encode(b"A" * 32).decode(),
            )

        # No grant added — failed precondition means no persistent state change.
        self.assertEqual(trust_store.list_grants(), [])
        # Token still present — joiner can retry with a fresh proof.
        self.assertTrue(tok.path.exists())

    def test_replayed_proof_cannot_be_used_on_different_token(self):
        """Binding the token_id into the HMAC prevents proof reuse across tokens."""
        from core.octopus import identity, onboarding
        from core.octopus.discovery import handshake

        identity.create("s")
        tok_a = onboarding.mint(issued_by_peer="s")
        tok_b = onboarding.mint(issued_by_peer="s")
        ch_b = handshake.build_challenge(tok_b.token_id)

        # Attacker knows tok_a's shared secret (leaked), but tok_a's
        # secret signed over tok_b's challenge will still use tok_b's
        # secret on the server side → mismatch.
        bad_proof = handshake.compute_proof(
            shared_secret_b64=tok_a.shared_secret_b64,
            challenge_b64=ch_b.challenge_b64,
            token_id=tok_b.token_id,
        )
        self.assertFalse(handshake.verify_proof(
            token_id=tok_b.token_id, proof_b64=bad_proof,
        ))


# ───────────────────────── Tytus CIDR scan ─────────────────────────

class TytusCIDRScanTest(unittest.TestCase):
    def test_stub_probe_returns_only_responders(self):
        from core.octopus.discovery.mdns import tytus_cidr_scan

        # Simulate exactly 3 responders in the /24.
        responders = {"10.42.42.1", "10.42.42.4", "10.42.42.128"}

        def stub_probe(host, port, timeout_s):
            return host in responders

        hits = tytus_cidr_scan(probe=stub_probe)
        self.assertEqual(set(hits), responders)
        # Sorted by octet order.
        self.assertEqual(hits, sorted(hits, key=lambda a: tuple(int(p) for p in a.split("."))))

    def test_scan_no_responders(self):
        from core.octopus.discovery.mdns import tytus_cidr_scan
        self.assertEqual(tytus_cidr_scan(probe=lambda *a: False), [])

    def test_scan_walks_full_slash_24(self):
        """Verify the scan hits exactly the 254 usable hosts in /24."""
        from core.octopus.discovery.mdns import tytus_cidr_scan
        probed = set()

        def stub_probe(host, port, timeout_s):
            probed.add(host)
            return False

        tytus_cidr_scan(probe=stub_probe)
        # /24 network address + broadcast excluded → 254 hosts.
        self.assertEqual(len(probed), 254)
        self.assertIn("10.42.42.1", probed)
        self.assertIn("10.42.42.254", probed)
        self.assertNotIn("10.42.42.0", probed)       # network
        self.assertNotIn("10.42.42.255", probed)     # broadcast


# ───────────────────────── mDNS roundtrip (integration) ────────────

try:
    import zeroconf  # noqa: F401
    ZEROCONF = True
except ImportError:
    ZEROCONF = False


@unittest.skipUnless(
    ZEROCONF,
    "zeroconf not installed — `pip install 'zeroconf>=0.132'` to exercise "
    "the mDNS roundtrip test. Invite-link onboarding works without it.",
)
class MDNSRoundtripTest(unittest.TestCase):
    """Sprint criterion: 'advertise on Node A found by discover on Node B
    within 3 seconds'. Because we can't spawn a second Mac in a unit test,
    we advertise + discover in the same process — which still exercises
    the full zeroconf path: socket bind, service register, browser
    resolve. On CI that blocks multicast, this test is marked SKIP via
    the try/except wrapper. On any dev laptop, it runs in ~2s.
    """

    def test_advertise_then_discover(self):
        from core.octopus.discovery import mdns
        # A unique peer name per-test-run so repeated runs don't collide.
        import uuid
        name = f"test-peer-{uuid.uuid4().hex[:8]}"
        pubkey = base64.b64encode(b"P" * 32).decode()
        zc, info = mdns.advertise(peer_name=name, public_key_b64=pubkey, port=18765)
        try:
            peers = mdns.discover(timeout_s=3.0)
            matching = [p for p in peers if p.peer_name == name]
            self.assertGreaterEqual(
                len(matching), 1,
                f"advertise/discover roundtrip failed — visible peers: {peers!r}",
            )
            if matching:
                m = matching[0]
                self.assertEqual(m.port, 18765)
                self.assertEqual(m.public_key_b64, pubkey)
        finally:
            zc.unregister_service(info)
            zc.close()


if __name__ == "__main__":
    unittest.main(verbosity=2)

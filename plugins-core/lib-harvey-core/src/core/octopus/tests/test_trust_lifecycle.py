#!/usr/bin/env python3
"""
Unit tests for Phase 2 trust lifecycle.

Covers:
  - Onboarding expiry: join fails when the token is > 1h old.
  - Grant persistence: TrustGrant survives a process restart (simulated
    by re-loading the module) and the consumed OnboardingToken is purged.
  - Revocation: immediately removes the peer from trusted.keys so the
    shim's mtime cache sees the peer as unknown on next call.
  - Identity lifecycle: create + load + sign round-trip.
  - Duration grammar: 30m/1h/24h/7d/permanent accepted, garbage rejected.
  - Shim trust file stays sorted + scoped to active grants only.

Isolation: every test scopes MAKAKOO_HOME to a fresh TemporaryDirectory,
so the on-disk octopus state doesn't leak into ~/.makakoo/.
"""

from __future__ import annotations

import base64
import importlib
import os
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve()
SRC_ROOT = HERE.parents[4] / "src"
sys.path.insert(0, str(SRC_ROOT))


def _reload_octopus_modules():
    """Reload the octopus package after MAKAKOO_HOME changes.

    The ``core.octopus`` package reads ``MAKAKOO_HOME`` at import time to
    resolve its path constants. Tests that scope the env variable to a
    tmpdir need to reload the package so the subpaths (IDENTITY_PATH,
    TRUST_STORE_PATH, etc.) are recomputed.
    """
    for name in list(sys.modules):
        if name == "core.octopus" or name.startswith("core.octopus."):
            del sys.modules[name]


class TrustLifecycleTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        os.environ["MAKAKOO_HOME"] = self.tmp.name
        _reload_octopus_modules()

    # ─────────────────────── identity ──────────────────────────────

    def test_identity_create_load_roundtrip(self):
        from core.octopus import identity
        ident = identity.create("sebastian-mbp")
        self.assertTrue(identity.exists())
        loaded = identity.load()
        self.assertEqual(loaded.peer_name, "sebastian-mbp")
        self.assertEqual(loaded.public_key_b64, ident.public_key_b64)
        # Round-trip: sign a message with the loaded key, verify with
        # its public half — proves persistence didn't corrupt bytes.
        sig = loaded.sign(b"hello")
        loaded.public_key().verify(sig, b"hello")

    def test_identity_create_refuses_overwrite_without_force(self):
        from core.octopus import identity
        identity.create("peer-a")
        with self.assertRaises(FileExistsError):
            identity.create("peer-b")
        identity.create("peer-b", overwrite=True)
        self.assertEqual(identity.load().peer_name, "peer-b")

    def test_identity_file_mode_600(self):
        from core.octopus import identity
        identity.create("peer-a")
        st = os.stat(identity.IDENTITY_PATH)
        # On POSIX, chmod 600 is 0o100600 in st_mode. On platforms that
        # don't honor chmod (WSL/NTFS), skip — identity module silently
        # tolerates and the bootstrap wizard surfaces a warning.
        if os.name == "posix":
            self.assertEqual(st.st_mode & 0o777, 0o600, f"identity mode {oct(st.st_mode)}")

    # ─────────────────────── onboarding ────────────────────────────

    def test_onboarding_token_mints_with_expiry(self):
        from core.octopus import onboarding
        tok = onboarding.mint(issued_by_peer="host", expires_in_s=60)
        self.assertEqual(tok.issued_by_peer, "host")
        self.assertFalse(tok.is_expired())
        # 32-byte random secret → 44 chars base64
        self.assertEqual(len(base64.b64decode(tok.shared_secret_b64)), 32)

    def test_onboarding_redeem_fails_on_expired_token(self):
        from core.octopus import onboarding
        tok = onboarding.mint(issued_by_peer="host", expires_in_s=60)
        # Past-dated redemption simulates "> 1h old" by bumping `now`.
        future = tok.expires_at_unix + 1
        with self.assertRaises(PermissionError):
            onboarding.redeem(tok.token_id, now=future)
        # Expired tokens are cleaned up on the failed redeem attempt.
        self.assertFalse(tok.path.exists(), "expired token should be GC'd")

    def test_onboarding_redeem_consumes_token(self):
        from core.octopus import onboarding
        tok = onboarding.mint(issued_by_peer="host", expires_in_s=60)
        self.assertTrue(tok.path.exists())
        onboarding.redeem(tok.token_id)
        self.assertFalse(tok.path.exists(), "redeemed token should be unlinked")
        # Second redeem → FileNotFoundError (single-use).
        with self.assertRaises(FileNotFoundError):
            onboarding.redeem(tok.token_id)

    def test_onboarding_duration_grammar(self):
        from core.octopus import onboarding
        for good in ("30m", "1h", "24h", "7d", "permanent"):
            tok = onboarding.mint(issued_by_peer="h", duration_default=good)
            self.assertEqual(tok.duration_default, good)
        for bad in ("0m", "abc", "1", "1w", "-5h", ""):
            with self.assertRaises(ValueError, msg=f"expected reject of {bad!r}"):
                onboarding.mint(issued_by_peer="h", duration_default=bad)

    def test_onboarding_rejects_bad_scope(self):
        from core.octopus import onboarding
        with self.assertRaises(ValueError):
            onboarding.mint(issued_by_peer="h", capability_scope="admin")

    # ─────────────────────── trust store ───────────────────────────

    def test_trust_grant_persists_across_module_reload(self):
        """Sprint criterion: 'TrustGrant remains after session restart'."""
        from core.octopus import identity, onboarding, trust_store

        ident = identity.create("host-1")
        tok = onboarding.mint(issued_by_peer=ident.peer_name, duration_default="permanent")
        # Promote via the full redeem → add_grant path so we exercise
        # the intended flow, not just trust_store in isolation.
        redeemed = onboarding.redeem(tok.token_id)
        trust_store.add_grant(
            peer_name="peer-A",
            public_key_b64=ident.public_key_b64,  # any valid 32-byte pubkey
            capability_scope=redeemed.capability_scope,
            granted_by_token_id=redeemed.token_id,
            duration=redeemed.duration_default,
        )

        # Simulated restart — drop all cached modules, reload with the
        # same MAKAKOO_HOME. State must survive.
        _reload_octopus_modules()
        from core.octopus import trust_store as ts2, onboarding as ob2

        grants = ts2.list_grants()
        self.assertEqual(len(grants), 1)
        self.assertEqual(grants[0].peer_name, "peer-A")

        # Token was consumed by redeem → must be gone on the rehydrated view too.
        self.assertEqual(ob2.list_active(), [])

    def test_trust_grant_duration_computes_expiry(self):
        from core.octopus import identity, trust_store

        ident = identity.create("h")
        grant = trust_store.add_grant(
            peer_name="peer-A",
            public_key_b64=ident.public_key_b64,
            capability_scope="read-brain",
            granted_by_token_id=None,
            duration="24h",
        )
        self.assertIsNotNone(grant.expires_at_unix)
        # 24h = 86400s. Tolerate a couple of seconds for test runtime.
        delta = grant.expires_at_unix - grant.granted_at_unix
        self.assertEqual(delta, 24 * 3600)

    def test_trust_grant_refuses_duplicate_active(self):
        from core.octopus import identity, trust_store
        ident = identity.create("h")
        trust_store.add_grant(
            peer_name="peer-A", public_key_b64=ident.public_key_b64,
            capability_scope="read-brain", granted_by_token_id=None, duration="permanent",
        )
        with self.assertRaises(ValueError):
            trust_store.add_grant(
                peer_name="peer-A", public_key_b64=ident.public_key_b64,
                capability_scope="full-brain", granted_by_token_id=None, duration="permanent",
            )

    def test_trust_grant_rejects_bad_pubkey(self):
        from core.octopus import trust_store
        with self.assertRaises(ValueError):
            trust_store.add_grant(
                peer_name="x", public_key_b64="not-base64!",
                capability_scope="read-brain", granted_by_token_id=None, duration="permanent",
            )
        with self.assertRaises(ValueError):
            trust_store.add_grant(
                peer_name="x", public_key_b64=base64.b64encode(b"\x00" * 16).decode(),
                capability_scope="read-brain", granted_by_token_id=None, duration="permanent",
            )

    # ─────────────────────── revocation ───────────────────────────

    def test_revoke_removes_peer_from_shim_trust_file(self):
        """Sprint criterion: 'revoke ... immediately results in 403 for
        subsequent requests'. We verify that the peer is no longer in
        trusted.keys after revoke — the shim's mtime cache picks this
        up on next request (< 1 s)."""
        from core.octopus import identity, trust_store, SHIM_TRUST_FILE

        ident = identity.create("h")
        trust_store.add_grant(
            peer_name="peer-A", public_key_b64=ident.public_key_b64,
            capability_scope="write-brain", granted_by_token_id=None, duration="permanent",
        )
        trust_store.add_grant(
            peer_name="peer-B", public_key_b64=ident.public_key_b64,
            capability_scope="read-brain", granted_by_token_id=None, duration="permanent",
        )
        # Pre-revoke: both peers visible to shim.
        contents = SHIM_TRUST_FILE.read_text()
        self.assertIn("peer-A ", contents)
        self.assertIn("peer-B ", contents)

        self.assertTrue(trust_store.revoke("peer-A", reason="quit team"))

        # Post-revoke: A gone, B remains.
        contents = SHIM_TRUST_FILE.read_text()
        self.assertNotIn("peer-A ", contents)
        self.assertIn("peer-B ", contents)

        # Audit trail preserved.
        all_grants = trust_store.list_grants(include_revoked=True)
        revoked = [g for g in all_grants if g.peer_name == "peer-A"][0]
        self.assertIsNotNone(revoked.revoked_at_unix)
        self.assertEqual(revoked.revoke_reason, "quit team")

        # Idempotent: second revoke is not an error, but returns False.
        self.assertFalse(trust_store.revoke("peer-A"))

    def test_shim_trust_file_is_sorted(self):
        """Diffability: sorted order so `git diff trusted.keys` is sane."""
        from core.octopus import identity, trust_store, SHIM_TRUST_FILE
        ident = identity.create("h")
        for peer in ("zebra", "alpha", "mike"):
            trust_store.add_grant(
                peer_name=peer, public_key_b64=ident.public_key_b64,
                capability_scope="read-brain", granted_by_token_id=None, duration="permanent",
            )
        lines = SHIM_TRUST_FILE.read_text().splitlines()
        names = [ln.split()[0] for ln in lines if ln.strip()]
        self.assertEqual(names, ["alpha", "mike", "zebra"])

    def test_revoke_unknown_peer_is_noop(self):
        from core.octopus import trust_store
        self.assertFalse(trust_store.revoke("ghost"))


class InviteLinkTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        os.environ["MAKAKOO_HOME"] = self.tmp.name
        _reload_octopus_modules()

    def test_invite_link_roundtrip(self):
        from core.octopus import identity, onboarding
        from core.octopus.bootstrap_wizard import (
            _encode_invite_link, _decode_invite_link, _resolve_token_id,
        )
        ident = identity.create("host-mac")
        tok = onboarding.mint(
            issued_by_peer=ident.peer_name,
            proposed_peer_name="peer-sarah",
            capability_scope="write-brain",
            duration_default="permanent",
        )
        link = _encode_invite_link(tok, ident)
        self.assertTrue(link.startswith("makakoo://join?t="))

        decoded = _decode_invite_link(link)
        self.assertEqual(decoded["tid"], tok.token_id)
        self.assertEqual(decoded["sec"], tok.shared_secret_b64)
        self.assertEqual(decoded["iss"], "host-mac")
        self.assertEqual(decoded["iss_pk"], ident.public_key_b64)
        self.assertEqual(decoded["peer"], "peer-sarah")

        self.assertEqual(_resolve_token_id(link), tok.token_id)
        self.assertEqual(_resolve_token_id(tok.token_id), tok.token_id)

    def test_invite_link_rejects_other_schemes(self):
        from core.octopus.bootstrap_wizard import _decode_invite_link
        with self.assertRaises(ValueError):
            _decode_invite_link("http://join?t=xxxx")
        with self.assertRaises(ValueError):
            _decode_invite_link("makakoo://join?x=xxxx")


if __name__ == "__main__":
    unittest.main(verbosity=2)

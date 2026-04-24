"""Cryptographic challenge-response handshake for onboarding → trust elevation.

The invite URL carries the token's shared secret, so the handshake is a
straightforward HMAC proof: the joiner proves knowledge of the secret
by returning ``HMAC-SHA256(shared_secret, challenge)`` for a fresh
server-issued challenge. On success, both sides exchange Ed25519
public keys and promote to a persistent :class:`TrustGrant`.

The spec intentionally runs over the Phase 1 HTTP shim using a pair of
new JSON-RPC methods (``octopus/handshake_challenge`` and
``octopus/handshake_complete``). These are dispatched inside
:mod:`core.mcp.http_shim`'s intercept layer (extension work) but the
*crypto* lives here, in a transport-agnostic shape, so the same
primitives can back a future websocket or UDP onboarding path.

Phase 2 shipped the local-redemption path (``onboarding.redeem()``
consumes the token and ``trust_store.add_grant()`` records the peer).
Phase 3 adds:

    - :func:`build_challenge` — server-side challenge mint.
    - :func:`compute_proof`   — client-side HMAC proof of secret knowledge.
    - :func:`verify_proof`    — server-side constant-time HMAC check.
    - :func:`complete_handshake` — one-shot orchestration that wraps
                                    token redemption + grant creation.

Flow (joiner = C, issuer = S):

    C: makakoo octopus join <link>
        decodes invite → (token_id, shared_secret, iss_pk)
        generates own keypair if absent (bootstrap)
        POST /rpc {method: octopus/handshake_challenge, params: {token_id}}
    S: verifies token_id exists + not expired
       mints a random 32-byte challenge, stashes (token_id → challenge, ttl=60s)
       returns {challenge_b64}
    C: proof = HMAC-SHA256(shared_secret, challenge || token_id)
       POST /rpc {method: octopus/handshake_complete,
                  params: {token_id, proof_b64,
                           claimed_peer_name, claimed_pubkey_b64}}
    S: loads onboarding token, recomputes proof, compares in constant time.
       On match: trust_store.add_grant(...) + onboarding.redeem(...).
       Returns {peer_name, scope, expires_at_unix}.

Constant-time comparison prevents a malicious peer from recovering the
shared secret via timing side-channels on the proof verification. The
challenge is stashed with a short TTL (60s) so replays past the window
are rejected even if the challenge leaks.
"""

from __future__ import annotations

import base64
import hashlib
import hmac
import os
import secrets
import time
from dataclasses import dataclass

from .. import onboarding, trust_store

CHALLENGE_TTL_S = 60
"""Seconds the server holds a challenge in memory before it's invalid.
Short enough that a replay attack needs an active network MitM; long
enough that a human-in-the-loop wizard has breathing room to complete."""

PROOF_DOMAIN_SEPARATOR = b"makakoo-octopus-handshake-v1|"
"""Prepended to every HMAC input so a proof minted in one domain can't
be replayed in another. Keeps future handshake variants safely
distinguishable (``-v2`` / ``-qr``, etc.) at the crypto layer."""


# ────────────────────────── types ──────────────────────────────────

@dataclass(frozen=True)
class Challenge:
    token_id: str
    challenge_b64: str
    issued_at_unix: int

    def is_expired(self, now: int | None = None) -> bool:
        now = int(time.time()) if now is None else now
        return (now - self.issued_at_unix) > CHALLENGE_TTL_S


@dataclass(frozen=True)
class HandshakeResult:
    peer_name: str
    capability_scope: str
    expires_at_unix: int | None


# ────────────────────────── in-memory challenge store ──────────────
#
# We keep challenges in a process-local dict. They're ephemeral by
# design (TTL 60 s, single-use) — persisting them to disk would add
# complexity without fixing anything, since the onboarding token
# itself is already single-use: a crashed server before
# complete_handshake means the joiner simply retries the entire flow.

_challenges: dict[str, Challenge] = {}


# ────────────────────────── server side ─────────────────────────────

def build_challenge(token_id: str, *, now: int | None = None) -> Challenge:
    """Mint a fresh challenge for ``token_id`` and stash it.

    Raises:
        FileNotFoundError: unknown token_id.
        PermissionError:   token already expired.
    """
    now = int(time.time()) if now is None else now
    tok = onboarding.load(token_id)  # FileNotFoundError if missing
    if tok.is_expired(now):
        raise PermissionError(
            f"onboarding token {token_id} expired at unix={tok.expires_at_unix}"
        )

    challenge_bytes = secrets.token_bytes(32)
    challenge_b64 = base64.b64encode(challenge_bytes).decode("ascii")
    ch = Challenge(token_id=token_id, challenge_b64=challenge_b64, issued_at_unix=now)
    _challenges[token_id] = ch
    return ch


def verify_proof(
    *,
    token_id: str,
    proof_b64: str,
    now: int | None = None,
) -> bool:
    """Verify the joiner's HMAC proof against the stashed challenge.

    Uses :func:`hmac.compare_digest` for constant-time comparison so
    verification timing doesn't leak info about the shared secret.

    Returns True iff:
      - we issued a challenge for this token_id and it hasn't expired;
      - the onboarding token still exists on disk (wasn't redeemed
        by another concurrent handshake attempt);
      - HMAC(secret, DOMAIN || challenge || token_id) matches proof_b64.

    Returning False on any failure (no log of which) denies a remote
    peer the oracle to distinguish "unknown token" from "bad proof".
    """
    now = int(time.time()) if now is None else now
    ch = _challenges.get(token_id)
    if ch is None:
        return False
    if ch.is_expired(now):
        _challenges.pop(token_id, None)
        return False

    try:
        tok = onboarding.load(token_id)
    except FileNotFoundError:
        _challenges.pop(token_id, None)
        return False

    expected = compute_proof(
        shared_secret_b64=tok.shared_secret_b64,
        challenge_b64=ch.challenge_b64,
        token_id=token_id,
    )
    try:
        proof_raw = base64.b64decode(proof_b64)
    except Exception:
        return False

    try:
        expected_raw = base64.b64decode(expected)
    except Exception:
        return False

    return hmac.compare_digest(proof_raw, expected_raw)


def complete_handshake(
    *,
    token_id: str,
    proof_b64: str,
    claimed_peer_name: str,
    claimed_pubkey_b64: str,
    now: int | None = None,
) -> HandshakeResult:
    """End-to-end server-side handshake finalizer.

    On success: adds a TrustGrant for the joiner, redeems (unlinks)
    the onboarding token, purges the challenge. On failure: raises
    without mutating persistent state — the token stays on disk so
    the joiner can retry (up to the token's 1h expiry).

    Raises:
        PermissionError: proof invalid, challenge expired, token expired.
        ValueError: pubkey malformed (propagated from trust_store).
    """
    now = int(time.time()) if now is None else now

    if not verify_proof(token_id=token_id, proof_b64=proof_b64, now=now):
        raise PermissionError("handshake proof invalid or expired")

    # At this point the token existed at verify_proof time — load it
    # again here for the scope/duration metadata. The redeem call is
    # last so mutation happens only after every precondition passes.
    tok = onboarding.load(token_id)

    grant = trust_store.add_grant(
        peer_name=claimed_peer_name,
        public_key_b64=claimed_pubkey_b64,
        capability_scope=tok.capability_scope,
        granted_by_token_id=token_id,
        duration=tok.duration_default,
        now=now,
    )

    # Burn the token + the challenge so a stolen proof can't be replayed.
    try:
        onboarding.redeem(token_id, now=now)
    except FileNotFoundError:
        pass
    _challenges.pop(token_id, None)

    return HandshakeResult(
        peer_name=grant.peer_name,
        capability_scope=grant.capability_scope,
        expires_at_unix=grant.expires_at_unix,
    )


# ────────────────────────── client side ─────────────────────────────

def compute_proof(
    *,
    shared_secret_b64: str,
    challenge_b64: str,
    token_id: str,
) -> str:
    """Client-side HMAC proof of shared-secret knowledge.

    ``HMAC-SHA256(secret, DOMAIN || challenge || token_id)``, base64.

    ``token_id`` is included in the HMAC input to bind the proof to
    this specific onboarding token — an attacker who observes a proof
    for token A cannot replay it against token B even if they could
    induce the server to issue the same challenge.
    """
    secret = base64.b64decode(shared_secret_b64)
    challenge = base64.b64decode(challenge_b64)
    mac = hmac.new(secret, None, hashlib.sha256)
    mac.update(PROOF_DOMAIN_SEPARATOR)
    mac.update(challenge)
    mac.update(token_id.encode("utf-8"))
    return base64.b64encode(mac.digest()).decode("ascii")


# ────────────────────────── test-only reset ─────────────────────────

def _reset_challenge_store() -> None:
    """Clear the in-memory challenge dict. Tests only — never call in
    production; existing handshakes in flight would silently fail."""
    _challenges.clear()

"""Short-lived onboarding tokens for the ``makakoo octopus invite/join`` handshake.

An onboarding token is a single-use, 1-hour-expiry ticket that lets a
prospective peer authenticate their first handshake WITHOUT holding a
persistent trust grant yet. The handshake elevates the onboarding token
into a :class:`TrustGrant` (see :mod:`core.octopus.trust_store`) by
swapping permanent Ed25519 public keys under HMAC-SHA256 using the
token's shared secret.

Why a separate layer from :mod:`trust_store`:
    Trust grants are long-lived (default permanent) and keyed on public
    keys; onboarding tokens are ephemeral and keyed on a 32-byte random
    id. Tying them together would conflate "I want to let this peer in
    once" with "I have a relationship with this peer" — which makes
    revocation messy (revoking an old peer would also revoke every
    expired onboarding token that named them). Keep them separate; the
    wizard only ever promotes one to the other after a successful
    handshake.

Storage:
    One token per file under ``$MAKAKOO_HOME/keys/onboarding/<token-id>.json``
    (chmod 600). File-per-token keeps the redemption path race-free:
    redemption is ``os.unlink`` on the file, which is atomic and
    cross-process safe — no shared mutable registry to lock.

Token doc shape (chmod 600):

    {
        "version": 1,
        "token_id": "<base64url 32 bytes>",
        "shared_secret_b64": "<base64 32 bytes>",
        "proposed_peer_name": "<optional-hint>",
        "capability_scope": "read-brain|write-brain|full-brain",
        "duration_default": "permanent",
        "created_at_unix": ...,
        "expires_at_unix": ...,
        "issued_by_peer": "<local identity peer_name>"
    }

The token's shared_secret is what the prospective peer proves knowledge
of during handshake — they HMAC the challenge with it, and we verify
locally using the same secret read from this file. The secret never
leaves disk in plaintext on the wire; only the handshake HMAC does.
"""

from __future__ import annotations

import base64
import json
import os
import secrets
import time
from dataclasses import dataclass
from pathlib import Path

from . import CAPABILITIES, OCTOPUS_KEYS_DIR, ONBOARDING_DIR

ONBOARDING_DOC_VERSION = 1
DEFAULT_EXPIRY_S = 3600  # 1 hour — sprint criterion


@dataclass(frozen=True)
class OnboardingToken:
    token_id: str
    shared_secret_b64: str
    proposed_peer_name: str | None
    capability_scope: str
    duration_default: str
    created_at_unix: int
    expires_at_unix: int
    issued_by_peer: str

    @property
    def path(self) -> Path:
        return ONBOARDING_DIR / f"{self.token_id}.json"

    def is_expired(self, now: int | None = None) -> bool:
        now = int(time.time()) if now is None else now
        return now >= self.expires_at_unix


def _ensure_dir() -> None:
    ONBOARDING_DIR.mkdir(parents=True, exist_ok=True)
    try:
        os.chmod(ONBOARDING_DIR, 0o700)
    except OSError:
        pass


def _b64u_random(n_bytes: int) -> str:
    return base64.urlsafe_b64encode(secrets.token_bytes(n_bytes)).rstrip(b"=").decode("ascii")


def _validate_scope(scope: str) -> str:
    if scope not in CAPABILITIES:
        raise ValueError(f"capability scope must be one of {CAPABILITIES}, got {scope!r}")
    return scope


def _validate_duration(duration: str) -> str:
    """Accept the same grammar as the existing write-access grant path —
    ``30m | 1h | 24h | 7d | permanent``. Anything else rejected at the
    API boundary so a typo'd invite can never burn a trust_store slot."""
    if duration == "permanent":
        return duration
    if not duration or len(duration) < 2:
        raise ValueError(f"invalid duration {duration!r}")
    unit = duration[-1]
    if unit not in "mhd":
        raise ValueError(f"invalid duration unit {unit!r} in {duration!r}")
    try:
        n = int(duration[:-1])
    except ValueError:
        raise ValueError(f"invalid duration number in {duration!r}")
    if n < 1:
        raise ValueError(f"duration number must be ≥ 1, got {duration!r}")
    return duration


def mint(
    *,
    issued_by_peer: str,
    proposed_peer_name: str | None = None,
    capability_scope: str = "write-brain",
    duration_default: str = "permanent",
    expires_in_s: int = DEFAULT_EXPIRY_S,
) -> OnboardingToken:
    """Mint a new single-use onboarding token and persist it.

    Args:
        issued_by_peer: ``peer_name`` of the host issuing the token (the
            one that will promote the handshake into a TrustGrant). Usually
            ``core.octopus.identity.load().peer_name``.
        proposed_peer_name: optional hint for the name of the peer that
            will redeem this token. The peer may override.
        capability_scope: ``read-brain``, ``write-brain``, or
            ``full-brain``. Default ``write-brain`` matches the common
            case for pods and SME teammates.
        duration_default: how long the promoted TrustGrant should last.
            Defaults to ``permanent`` because the sprint's #1 use case is
            SME teammates that never re-onboard.
        expires_in_s: seconds until the ONBOARDING token itself expires.
            Does not affect the duration of the eventual TrustGrant.
    """
    _ensure_dir()
    _validate_scope(capability_scope)
    _validate_duration(duration_default)

    token_id = _b64u_random(32)
    shared_secret_b64 = base64.b64encode(secrets.token_bytes(32)).decode("ascii")
    now = int(time.time())

    doc = {
        "version": ONBOARDING_DOC_VERSION,
        "token_id": token_id,
        "shared_secret_b64": shared_secret_b64,
        "proposed_peer_name": proposed_peer_name,
        "capability_scope": capability_scope,
        "duration_default": duration_default,
        "created_at_unix": now,
        "expires_at_unix": now + int(expires_in_s),
        "issued_by_peer": issued_by_peer,
    }

    path = ONBOARDING_DIR / f"{token_id}.json"
    tmp = path.with_name(path.name + ".tmp")
    with tmp.open("w") as f:
        json.dump(doc, f, indent=2, sort_keys=True)
        f.write("\n")
    try:
        os.chmod(tmp, 0o600)
    except OSError:
        pass
    os.replace(tmp, path)

    return _doc_to_token(doc)


def _doc_to_token(doc: dict) -> OnboardingToken:
    if doc.get("version") != ONBOARDING_DOC_VERSION:
        raise RuntimeError(f"onboarding doc version {doc.get('version')!r} unsupported")
    return OnboardingToken(
        token_id=doc["token_id"],
        shared_secret_b64=doc["shared_secret_b64"],
        proposed_peer_name=doc.get("proposed_peer_name"),
        capability_scope=doc["capability_scope"],
        duration_default=doc["duration_default"],
        created_at_unix=int(doc["created_at_unix"]),
        expires_at_unix=int(doc["expires_at_unix"]),
        issued_by_peer=doc["issued_by_peer"],
    )


def load(token_id: str) -> OnboardingToken:
    """Load a token by id. Raises :class:`FileNotFoundError` if absent."""
    path = ONBOARDING_DIR / f"{token_id}.json"
    with path.open("r") as f:
        doc = json.load(f)
    return _doc_to_token(doc)


def redeem(token_id: str, now: int | None = None) -> OnboardingToken:
    """Load + validate + CONSUME the token in one call.

    Semantic: after a successful handshake the wizard calls redeem() to
    (a) prove the token existed and wasn't expired, (b) remove it from
    disk so it can't be replayed. The order — load, validate, unlink — is
    intentional: unlink comes after validation so a replay attempt on an
    expired token doesn't silently delete it (the user would lose the
    audit trail of who tried to abuse an expired token).

    Raises:
        FileNotFoundError: token id unknown.
        PermissionError: token expired (also deletes the expired token
            on the way out — expired tokens are never resurrectable).
    """
    path = ONBOARDING_DIR / f"{token_id}.json"
    tok = load(token_id)  # FileNotFoundError bubbles up
    if tok.is_expired(now):
        # Garbage-collect the expired token. Don't raise first — we want
        # the on-disk state to reflect that the token is gone even if
        # the caller ignores the exception.
        try:
            path.unlink()
        except FileNotFoundError:
            pass
        raise PermissionError(
            f"onboarding token {token_id} expired at unix={tok.expires_at_unix}; "
            "ask the issuing peer for a fresh invite"
        )
    # Consume — unlink before returning so even if the caller crashes
    # between here and promoting the TrustGrant, the token is burned.
    path.unlink()
    return tok


def list_active(now: int | None = None) -> list[OnboardingToken]:
    """Return every non-expired token on disk.

    Bonus: garbage-collects expired tokens encountered during the walk,
    so periodic callers (``makakoo octopus trust list`` shows only valid
    invites) double as a janitor pass.
    """
    now = int(time.time()) if now is None else now
    out: list[OnboardingToken] = []
    if not ONBOARDING_DIR.exists():
        return out
    for p in ONBOARDING_DIR.iterdir():
        if not p.name.endswith(".json"):
            continue
        try:
            with p.open("r") as f:
                doc = json.load(f)
            tok = _doc_to_token(doc)
        except Exception:
            continue
        if tok.is_expired(now):
            try:
                p.unlink()
            except FileNotFoundError:
                pass
            continue
        out.append(tok)
    return out

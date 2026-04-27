"""Invite-link encoding + decoding for cross-network onboarding.

The invite-link URL format is ``makakoo://join?t=<base64url-json>``.
Payload shape is documented in :func:`encode_invite` below.

This module provides both the encoder (for the issuing host — also
used by :mod:`core.octopus.bootstrap_wizard`) and the decoder with
strict validation (for the joining host, which must refuse ill-formed
or wrong-scheme URLs before passing them to the handshake layer).

Keep this free of any side-effects — no on-disk writes. The wizard
handles persistence; this module handles serialization only.
"""

from __future__ import annotations

import base64
import json
import time
import urllib.parse
from dataclasses import dataclass

INVITE_URL_SCHEME = "makakoo"
INVITE_URL_HOST = "join"
INVITE_PAYLOAD_VERSION = 1


@dataclass(frozen=True)
class InvitePayload:
    """Typed wrapper over the decoded invite payload.

    Attrs are parallel to the JSON keys used in the URL. Keeping them
    a frozen dataclass lets the handshake layer pass these around
    without worrying about accidental mutation.
    """
    version: int
    token_id: str
    shared_secret_b64: str
    capability_scope: str
    duration_default: str
    expires_at_unix: int
    issuer_peer_name: str
    issuer_public_key_b64: str
    proposed_peer_name: str | None

    def is_expired(self, now: int | None = None) -> bool:
        now = int(time.time()) if now is None else now
        return now >= self.expires_at_unix


def encode_invite(
    *,
    token_id: str,
    shared_secret_b64: str,
    capability_scope: str,
    duration_default: str,
    expires_at_unix: int,
    issuer_peer_name: str,
    issuer_public_key_b64: str,
    proposed_peer_name: str | None = None,
) -> str:
    """Encode invite fields into a ``makakoo://join?t=<b64>`` URL.

    Field keys are compact (3-letter) to keep the URL short enough to
    fit in QR codes without spilling to alt-version-L — the full
    payload is ~250-280 chars base64, which encodes to a reliable
    QR version 10 (57x57).
    """
    payload = {
        "v": INVITE_PAYLOAD_VERSION,
        "tid": token_id,
        "sec": shared_secret_b64,
        "scope": capability_scope,
        "dur": duration_default,
        "exp": int(expires_at_unix),
        "iss": issuer_peer_name,
        "iss_pk": issuer_public_key_b64,
    }
    if proposed_peer_name:
        payload["peer"] = proposed_peer_name
    raw = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    b64 = base64.urlsafe_b64encode(raw).rstrip(b"=").decode("ascii")
    return f"{INVITE_URL_SCHEME}://{INVITE_URL_HOST}?t={b64}"


def decode_invite(link: str) -> InvitePayload:
    """Parse a ``makakoo://join?t=...`` URL into an :class:`InvitePayload`.

    Rejects URLs with wrong scheme or host with a clear ``ValueError``
    so the joining wizard can surface the problem to the user instead
    of attempting an onboarding request against an untrusted URL.

    Raises:
        ValueError: malformed URL, bad base64, missing required field,
            unsupported payload version.
    """
    parsed = urllib.parse.urlparse(link)
    if parsed.scheme != INVITE_URL_SCHEME:
        raise ValueError(
            f"not a makakoo invite URL: scheme={parsed.scheme!r} "
            f"(expected {INVITE_URL_SCHEME!r})"
        )
    if parsed.netloc != INVITE_URL_HOST:
        raise ValueError(
            f"not a makakoo invite URL: host={parsed.netloc!r} "
            f"(expected {INVITE_URL_HOST!r})"
        )

    qs = urllib.parse.parse_qs(parsed.query)
    b64 = qs.get("t", [""])[0]
    if not b64:
        raise ValueError("invite URL missing `t` query param")
    padded = b64 + "=" * ((4 - len(b64) % 4) % 4)
    try:
        raw = base64.urlsafe_b64decode(padded.encode("ascii"))
    except Exception as exc:
        raise ValueError(f"invite URL `t` parameter is not valid base64url: {exc}") from exc

    try:
        payload = json.loads(raw.decode("utf-8"))
    except Exception as exc:
        raise ValueError(f"invite URL payload is not valid JSON: {exc}") from exc

    version = payload.get("v")
    if version != INVITE_PAYLOAD_VERSION:
        raise ValueError(
            f"invite payload version {version!r} unsupported "
            f"(expected {INVITE_PAYLOAD_VERSION}); upgrade your makakoo install"
        )

    missing = [
        k for k in ("tid", "sec", "scope", "dur", "exp", "iss", "iss_pk")
        if not payload.get(k)
    ]
    if missing:
        raise ValueError(f"invite payload missing required field(s): {missing}")

    return InvitePayload(
        version=version,
        token_id=str(payload["tid"]),
        shared_secret_b64=str(payload["sec"]),
        capability_scope=str(payload["scope"]),
        duration_default=str(payload["dur"]),
        expires_at_unix=int(payload["exp"]),
        issuer_peer_name=str(payload["iss"]),
        issuer_public_key_b64=str(payload["iss_pk"]),
        proposed_peer_name=(
            str(payload["peer"]) if payload.get("peer") else None
        ),
    )

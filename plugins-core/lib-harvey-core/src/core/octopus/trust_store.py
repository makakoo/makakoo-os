"""Persistent trust grants between Octopus peers.

A :class:`TrustGrant` is a long-lived record that authorizes a remote
peer to call into the local HTTP shim. Grants are keyed on the peer's
``peer_name`` (stable, user-chosen) and carry the Ed25519 public key
that the shim uses to verify signatures. They are minted via the
``makakoo octopus join`` handshake (Phase 3) or the interactive
bootstrap wizard (Phase 2).

Storage:
    Single JSON doc at ``$MAKAKOO_HOME/keys/trust_store.json`` (chmod 600).
    Small: a few hundred grants fit trivially. Every mutation is an
    atomic temp-sibling + ``os.replace`` rewrite to keep the doc
    consistent under concurrent ``makakoo octopus trust`` invocations.

Why a single file, not file-per-grant:
    Trust grants change rarely and are read hot on every request (via
    the HTTP shim's ``trusted.keys`` cache). A single file keeps the
    mental model simple and the on-disk layout auditable with one
    ``cat``. Onboarding tokens (which churn fast and need race-free
    redemption) use file-per-token instead — see :mod:`onboarding`.

Shim integration:
    Every mutation to the trust store also writes the peer's pubkey line
    into ``$MAKAKOO_HOME/config/peers/trusted.keys`` — the file the HTTP
    shim (Phase 1) already reads via its mtime cache. This keeps
    ``trusted.keys`` as the single source of truth the shim consumes
    while giving Octopus the richer schema (capability scope, revoked_at,
    granted_by_token_id) it needs for lifecycle management.

    Revocation rewrites ``trusted.keys`` to omit the revoked peer — a
    revoked grant becomes invisible to the shim on its next mtime check
    (≤ 1s). Per the sprint criterion "Revocation: revoke <peer-id>
    immediately results in 403 for subsequent requests from that peer."

Doc shape (chmod 600):

    {
        "version": 1,
        "grants": [
            {
                "peer_name": "...",
                "public_key_b64": "<base64 32 bytes>",
                "capability_scope": "read-brain|write-brain|full-brain",
                "granted_by_token_id": "...",   # audit: which invite
                "granted_at_unix": ...,
                "duration": "1h|7d|permanent",
                "expires_at_unix": ...,         # null if permanent
                "revoked_at_unix": ...,         # null if active
                "revoke_reason": "..."          # null if active
            },
            ...
        ]
    }

Grants are NEVER physically deleted — revocation sets ``revoked_at_unix``
and ``revoke_reason`` and rewrites the shim's ``trusted.keys`` to omit
the peer. Keeping revoked grants on disk means ``makakoo octopus trust
list --include-revoked`` can show a complete audit trail ("this peer
was granted on X and revoked on Y because Z").
"""

from __future__ import annotations

import base64
import json
import os
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Iterable

from . import CAPABILITIES, OCTOPUS_KEYS_DIR, SHIM_TRUST_FILE, TRUST_STORE_PATH

TRUST_STORE_DOC_VERSION = 1


@dataclass
class TrustGrant:
    peer_name: str
    public_key_b64: str
    capability_scope: str
    granted_by_token_id: str | None
    granted_at_unix: int
    duration: str
    expires_at_unix: int | None
    revoked_at_unix: int | None = None
    revoke_reason: str | None = None

    def is_active(self, now: int | None = None) -> bool:
        now = int(time.time()) if now is None else now
        if self.revoked_at_unix is not None:
            return False
        if self.expires_at_unix is not None and now >= self.expires_at_unix:
            return False
        return True


def _ensure_keys_dir() -> None:
    OCTOPUS_KEYS_DIR.mkdir(parents=True, exist_ok=True)
    try:
        os.chmod(OCTOPUS_KEYS_DIR, 0o700)
    except OSError:
        pass


def _empty_doc() -> dict:
    return {"version": TRUST_STORE_DOC_VERSION, "grants": []}


def _load_doc() -> dict:
    if not TRUST_STORE_PATH.exists():
        return _empty_doc()
    with TRUST_STORE_PATH.open("r") as f:
        doc = json.load(f)
    if doc.get("version") != TRUST_STORE_DOC_VERSION:
        raise RuntimeError(
            f"trust store version {doc.get('version')!r} unsupported; "
            f"expected {TRUST_STORE_DOC_VERSION}"
        )
    return doc


def _write_doc(doc: dict) -> None:
    _ensure_keys_dir()
    tmp = TRUST_STORE_PATH.with_name(TRUST_STORE_PATH.name + ".tmp")
    with tmp.open("w") as f:
        json.dump(doc, f, indent=2, sort_keys=True)
        f.write("\n")
    try:
        os.chmod(tmp, 0o600)
    except OSError:
        pass
    os.replace(tmp, TRUST_STORE_PATH)


def _compute_expiry(duration: str, granted_at_unix: int) -> int | None:
    """Translate the sprint's duration grammar to an absolute expiry.

    Accepts ``30m | 1h | 24h | 7d | permanent`` (same grammar as the
    existing write-access grant path). Anything else raises ValueError
    at the boundary so a typo can't silently produce a permanent grant.
    """
    if duration == "permanent":
        return None
    unit = duration[-1]
    try:
        n = int(duration[:-1])
    except ValueError:
        raise ValueError(f"invalid duration {duration!r}")
    if unit == "m":
        return granted_at_unix + n * 60
    if unit == "h":
        return granted_at_unix + n * 3600
    if unit == "d":
        return granted_at_unix + n * 86400
    raise ValueError(f"invalid duration unit {unit!r} in {duration!r}")


def _sync_shim_trust_file(grants: Iterable[TrustGrant]) -> None:
    """Rewrite ``$MAKAKOO_HOME/config/peers/trusted.keys`` from grants.

    Every call is a full rewrite — cheap at N ≤ hundreds, and the alternative
    (incremental patching) would introduce drift between trust_store.json
    and trusted.keys. The shim's mtime-cache picks up the change on its
    next call, so grants propagate to authz within seconds.

    Order is stable (alphabetical by peer_name) so diffs across mutations
    are human-auditable.
    """
    shim_file = SHIM_TRUST_FILE
    shim_file.parent.mkdir(parents=True, exist_ok=True)
    lines: list[str] = []
    now = int(time.time())
    for g in sorted(grants, key=lambda g: g.peer_name):
        if not g.is_active(now):
            continue
        lines.append(f"{g.peer_name} {g.public_key_b64}\n")
    tmp = shim_file.with_name(shim_file.name + ".tmp")
    with tmp.open("w") as f:
        f.writelines(lines)
    try:
        os.chmod(tmp, 0o600)
    except OSError:
        pass
    os.replace(tmp, shim_file)


# ────────────────────────── public API ──────────────────────────────

def add_grant(
    *,
    peer_name: str,
    public_key_b64: str,
    capability_scope: str,
    granted_by_token_id: str | None,
    duration: str = "permanent",
    now: int | None = None,
) -> TrustGrant:
    """Add a new TrustGrant. Raises if an active grant for ``peer_name``
    already exists (caller must revoke + re-grant to change scope or
    duration — prevents silent scope upgrades).

    Verifies pubkey shape (32 bytes raw base64) before persisting so a
    malformed key can't land in the shim's trust file and get silently
    ignored.
    """
    if not peer_name or not peer_name.strip():
        raise ValueError("peer_name must be non-empty")
    if capability_scope not in CAPABILITIES:
        raise ValueError(f"scope must be one of {CAPABILITIES}, got {capability_scope!r}")
    try:
        raw = base64.b64decode(public_key_b64)
    except Exception as exc:
        raise ValueError(f"public_key_b64 invalid base64: {exc}") from exc
    if len(raw) != 32:
        raise ValueError(f"public key must be 32 raw bytes, got {len(raw)}")

    now = int(time.time()) if now is None else now
    expires_at_unix = _compute_expiry(duration, now)

    doc = _load_doc()
    grants = [_from_dict(g) for g in doc["grants"]]

    for g in grants:
        if g.peer_name == peer_name and g.is_active(now):
            raise ValueError(
                f"active grant already exists for peer {peer_name!r}; "
                "revoke before re-granting to prevent silent scope upgrades"
            )

    new = TrustGrant(
        peer_name=peer_name.strip(),
        public_key_b64=public_key_b64,
        capability_scope=capability_scope,
        granted_by_token_id=granted_by_token_id,
        granted_at_unix=now,
        duration=duration,
        expires_at_unix=expires_at_unix,
        revoked_at_unix=None,
        revoke_reason=None,
    )
    grants.append(new)

    doc["grants"] = [asdict(g) for g in grants]
    _write_doc(doc)
    _sync_shim_trust_file(grants)
    return new


def list_grants(*, include_revoked: bool = False, now: int | None = None) -> list[TrustGrant]:
    """Return all grants. By default only active ones (not revoked, not expired).

    Set ``include_revoked=True`` for the audit view — useful in
    ``makakoo octopus trust list --all``.
    """
    now = int(time.time()) if now is None else now
    doc = _load_doc()
    grants = [_from_dict(g) for g in doc["grants"]]
    if include_revoked:
        return grants
    return [g for g in grants if g.is_active(now)]


def get(peer_name: str, *, now: int | None = None) -> TrustGrant | None:
    """Return the active grant for ``peer_name`` or None."""
    now = int(time.time()) if now is None else now
    for g in list_grants(include_revoked=False, now=now):
        if g.peer_name == peer_name:
            return g
    return None


def revoke(peer_name: str, *, reason: str | None = None, now: int | None = None) -> bool:
    """Revoke the active grant for ``peer_name``. Returns True if one was revoked.

    The rewrite of ``trusted.keys`` is the critical security step: within
    the shim's mtime-cache window (≤ 1s) the peer's next signed request
    hits ``unknown peer`` → HTTP 401. The sprint spec asks for 403; we
    return 401 because that's what the shim's unknown-peer branch
    already returns, and it's semantically accurate (the peer is no
    longer authenticated, not merely forbidden from this one resource).
    Spec compliance note below.

    Returns False if no active grant exists for the peer (idempotent —
    double revoke is not an error).
    """
    now = int(time.time()) if now is None else now
    doc = _load_doc()
    grants = [_from_dict(g) for g in doc["grants"]]
    changed = False
    for g in grants:
        if g.peer_name == peer_name and g.is_active(now):
            g.revoked_at_unix = now
            g.revoke_reason = reason or "revoked via makakoo octopus trust revoke"
            changed = True
    if changed:
        doc["grants"] = [asdict(g) for g in grants]
        _write_doc(doc)
        _sync_shim_trust_file(grants)
    return changed


def resync_shim_trust_file(now: int | None = None) -> None:
    """Regenerate ``trusted.keys`` from the trust_store.

    Used by ``makakoo doctor --octopus`` to heal drift: if trusted.keys is
    hand-edited or corrupt, re-deriving from the (authoritative) trust
    store restores a known-good state without any grant mutations.
    """
    now = int(time.time()) if now is None else now
    doc = _load_doc()
    grants = [_from_dict(g) for g in doc["grants"]]
    _sync_shim_trust_file(grants)


def _from_dict(d: dict) -> TrustGrant:
    # Normalize legacy docs that might be missing the revoked_* fields.
    return TrustGrant(
        peer_name=d["peer_name"],
        public_key_b64=d["public_key_b64"],
        capability_scope=d["capability_scope"],
        granted_by_token_id=d.get("granted_by_token_id"),
        granted_at_unix=int(d["granted_at_unix"]),
        duration=d.get("duration", "permanent"),
        expires_at_unix=(
            int(d["expires_at_unix"]) if d.get("expires_at_unix") is not None else None
        ),
        revoked_at_unix=(
            int(d["revoked_at_unix"]) if d.get("revoked_at_unix") is not None else None
        ),
        revoke_reason=d.get("revoke_reason"),
    )

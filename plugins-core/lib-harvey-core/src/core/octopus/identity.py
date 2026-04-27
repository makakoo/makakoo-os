"""Ed25519 identity lifecycle for an Octopus peer.

Each host has ONE long-lived identity: a keypair whose private half is
persisted at ``$MAKAKOO_HOME/keys/octopus-identity.json`` (chmod 600) and
whose public half is distributed to peers via the invite flow (Phase 3)
or the legacy ``trusted.keys`` file (pre-Phase-2 paths still work).

The identity doc is intentionally tiny so it's easy to back up, rotate,
and migrate. Every field is a string so the doc is trivially editable by
hand in an emergency (e.g. copy-pasting a private key into a new install
after a disk failure):

    {
        "version": 1,
        "peer_name": "<host-or-user-chosen name, e.g. 'sebastian-macbook'>",
        "public_key_b64":  "<base64(32-byte raw Ed25519 pubkey)>",
        "private_key_b64": "<base64(32-byte raw Ed25519 seed)>",
        "created_at_unix": 1713950000
    }

``peer_name`` is the stable name this host advertises to the mesh. It's
the value other peers will add to their ``trusted.keys`` file and to
their ``trust_store.json``. Collision handling is out-of-scope for
Phase 2 — peer names are assumed unique across a user's mesh. Phase 3's
mDNS layer will surface collisions at discovery time.
"""

from __future__ import annotations

import base64
import json
import os
import time
from dataclasses import dataclass
from pathlib import Path

from cryptography.hazmat.primitives.asymmetric.ed25519 import (
    Ed25519PrivateKey,
    Ed25519PublicKey,
)

from . import IDENTITY_PATH, OCTOPUS_KEYS_DIR

IDENTITY_DOC_VERSION = 1


@dataclass(frozen=True)
class Identity:
    """In-memory handle to a loaded Ed25519 identity.

    Instances are constructed by :func:`load` (reads the on-disk doc) or
    :func:`create` (generates and persists a fresh keypair). The raw
    private seed is held on the instance; callers are responsible for not
    logging it. :func:`sign` and :func:`public_key_b64` are the two
    access patterns you should reach for first.
    """
    peer_name: str
    public_key_b64: str
    private_key_b64: str
    created_at_unix: int

    def private_key(self) -> Ed25519PrivateKey:
        return Ed25519PrivateKey.from_private_bytes(base64.b64decode(self.private_key_b64))

    def public_key(self) -> Ed25519PublicKey:
        return Ed25519PublicKey.from_public_bytes(base64.b64decode(self.public_key_b64))

    def sign(self, data: bytes) -> bytes:
        return self.private_key().sign(data)


def _ensure_keys_dir() -> None:
    """Create ``$MAKAKOO_HOME/keys/`` with 0o700 if missing.

    Deliberate: we want the parent dir locked down even if the user's
    umask is permissive. Any tighter mode (0o600) would break directory
    traversal — 0o700 is the minimum that lets us chmod files underneath.
    """
    OCTOPUS_KEYS_DIR.mkdir(parents=True, exist_ok=True)
    try:
        os.chmod(OCTOPUS_KEYS_DIR, 0o700)
    except OSError:
        # On platforms that don't honor chmod (WSL/NTFS), tolerate silently.
        pass


def exists() -> bool:
    return IDENTITY_PATH.exists()


def load() -> Identity:
    """Load the persisted identity. Raises :class:`FileNotFoundError` if absent."""
    with IDENTITY_PATH.open("r") as f:
        doc = json.load(f)
    if doc.get("version") != IDENTITY_DOC_VERSION:
        raise RuntimeError(
            f"identity doc version {doc.get('version')!r} unsupported; "
            f"expected {IDENTITY_DOC_VERSION}"
        )
    return Identity(
        peer_name=doc["peer_name"],
        public_key_b64=doc["public_key_b64"],
        private_key_b64=doc["private_key_b64"],
        created_at_unix=int(doc["created_at_unix"]),
    )


def create(peer_name: str, *, overwrite: bool = False) -> Identity:
    """Generate a new Ed25519 keypair and persist to disk.

    Args:
        peer_name: stable mesh-wide name for this host.
        overwrite: if True, replace an existing identity doc. The caller
                   is responsible for warning the user — rotating keys
                   invalidates every TrustGrant other peers hold for
                   this host.
    """
    if not peer_name or not peer_name.strip():
        raise ValueError("peer_name must be non-empty")
    if IDENTITY_PATH.exists() and not overwrite:
        raise FileExistsError(
            f"{IDENTITY_PATH} already exists; pass overwrite=True to rotate "
            "(this invalidates every existing TrustGrant on peer hosts)"
        )

    _ensure_keys_dir()
    sk = Ed25519PrivateKey.generate()
    sk_bytes = sk.private_bytes_raw()
    pk_bytes = sk.public_key().public_bytes_raw()

    doc = {
        "version": IDENTITY_DOC_VERSION,
        "peer_name": peer_name.strip(),
        "public_key_b64": base64.b64encode(pk_bytes).decode("ascii"),
        "private_key_b64": base64.b64encode(sk_bytes).decode("ascii"),
        "created_at_unix": int(time.time()),
    }

    _atomic_write_identity(doc)
    return Identity(
        peer_name=doc["peer_name"],
        public_key_b64=doc["public_key_b64"],
        private_key_b64=doc["private_key_b64"],
        created_at_unix=doc["created_at_unix"],
    )


def load_or_create(peer_name_hint: str) -> Identity:
    """Return the existing identity or create one using ``peer_name_hint``."""
    if IDENTITY_PATH.exists():
        return load()
    return create(peer_name_hint)


def _atomic_write_identity(doc: dict) -> None:
    """Write identity doc atomically + chmod 600.

    Atomicity matters because a half-written identity doc would brick a
    host: the doc is the ONLY copy of the private key. We write a temp
    sibling file, chmod, then rename — no window where a peer process
    could read an empty file.
    """
    _ensure_keys_dir()
    tmp = IDENTITY_PATH.with_name(IDENTITY_PATH.name + ".tmp")
    with tmp.open("w") as f:
        json.dump(doc, f, indent=2, sort_keys=True)
        f.write("\n")
    try:
        os.chmod(tmp, 0o600)
    except OSError:
        pass
    os.replace(tmp, IDENTITY_PATH)

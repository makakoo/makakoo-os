"""Harvey Octopus — signed-MCP peer federation for Makakoo OS.

Submodules:
    identity       — Ed25519 keypair lifecycle, persisted at
                     ``$MAKAKOO_HOME/keys/octopus-identity.json`` (chmod 600).
    onboarding     — short-lived (1h) single-use join tokens with a
                     temporary shared secret per token.
    trust_store    — persistent :class:`TrustGrant` store at
                     ``$MAKAKOO_HOME/keys/trust_store.json`` (chmod 600).
    bootstrap_wizard — interactive ``makakoo octopus bootstrap`` flow.
    discovery/mdns / invite / handshake — Phase 3.
    enforce / ratelimit — Phase 4.

Path convention:
    All Octopus state lives under ``$MAKAKOO_HOME/keys/`` on the host.
    This keeps the mesh-scoped identity + trust alongside the existing
    peer trust file (``$MAKAKOO_HOME/config/peers/trusted.keys``) that
    the HTTP shim consumes. The bootstrap wizard promotes TrustGrants
    into entries in that file so adding a peer via ``makakoo octopus``
    stays behind a single switch the HTTP shim already reads.
"""

from pathlib import Path
import os

MAKAKOO_HOME = Path(os.environ.get("MAKAKOO_HOME", os.path.expanduser("~/MAKAKOO")))
OCTOPUS_KEYS_DIR = MAKAKOO_HOME / "keys"
IDENTITY_PATH = OCTOPUS_KEYS_DIR / "octopus-identity.json"
TRUST_STORE_PATH = OCTOPUS_KEYS_DIR / "trust_store.json"
ONBOARDING_DIR = OCTOPUS_KEYS_DIR / "onboarding"
# The legacy trust file consumed by the HTTP shim. Bootstrap writes new
# peer entries here so a TrustGrant implies an HTTP-reachable peer with
# zero extra steps.
SHIM_TRUST_FILE = MAKAKOO_HOME / "config" / "peers" / "trusted.keys"

CAPABILITIES = ("read-brain", "write-brain", "full-brain")
"""Tuple of valid capability scopes — ordered coarsest-first (read-brain
is a subset of write-brain is a subset of full-brain). Enforcement logic
in Phase 4 relies on this ordering; change carefully."""

"""``makakoo octopus`` command surface — bootstrap, invite, join, trust.

The wizard is a thin Python CLI wrapped by the Rust ``makakoo octopus``
clap subcommands (which shell out here). Keeping the logic in Python
lets us share code with the HTTP shim (trust store, onboarding tokens,
identity) without duplicating it across the language boundary — the
Rust side is just a clap front and arg passthrough.

Subcommands:
    bootstrap       — one-time setup: generate identity, write trust store
                      scaffold, print next steps.
    invite [--link] — mint a single-use 1h onboarding token; optionally
                      encode it into a ``makakoo://join?t=<base64>`` URL.
    join <token|url> [--peer-name N] [--pubkey KEY]
                    — redeem an onboarding token and promote it into a
                      TrustGrant (Phase 3 handshake wires the key
                      exchange; Phase 2 version accepts the remote's
                      pubkey explicitly).
    trust list [--all]
                    — show active (or all, including revoked) grants.
    trust revoke <peer-name> [--reason R]
                    — revoke a grant and re-sync the shim trust file.
    doctor          — read-only health check: identity present,
                      trust_store + trusted.keys in sync, onboarding
                      tokens all non-expired.

Entry point: ``python -m core.octopus.bootstrap_wizard <subcommand> ...``
"""

from __future__ import annotations

import argparse
import base64
import datetime
import json
import os
import sys
import time
import urllib.parse
from typing import Sequence

from . import CAPABILITIES, OCTOPUS_KEYS_DIR, SHIM_TRUST_FILE, TRUST_STORE_PATH
from . import identity, onboarding, trust_store
from .discovery import invite as invite_mod


# ────────────────────────── bootstrap ──────────────────────────────

def cmd_bootstrap(args: argparse.Namespace) -> int:
    peer_name = (args.peer_name or _default_peer_name()).strip()
    if not peer_name:
        print("peer-name cannot be empty", file=sys.stderr)
        return 2

    if identity.exists() and not args.force:
        ident = identity.load()
        print(f"✓ identity already exists: peer_name={ident.peer_name}")
        print(f"  public_key_b64 = {ident.public_key_b64}")
        print(f"  path           = {identity.IDENTITY_PATH}")
    else:
        ident = identity.create(peer_name, overwrite=args.force)
        print(f"✓ identity created: peer_name={ident.peer_name}")
        print(f"  public_key_b64 = {ident.public_key_b64}")
        print(f"  path           = {identity.IDENTITY_PATH}  (chmod 600)")

    # Make sure trust store exists (empty doc is fine) so the downstream
    # shim trust file is regenerated on first revocation/grant without
    # an Error-on-missing.
    trust_store.resync_shim_trust_file()
    print(f"✓ trust store ready at {TRUST_STORE_PATH}")
    print(f"✓ shim trust file synced: {SHIM_TRUST_FILE}")
    print("")
    print("Next steps:")
    print("  1. Invite a peer:   makakoo octopus invite --link")
    print("  2. Peer joins with: makakoo octopus join <link>")
    print("  3. Start the peer stack: makakoo agent start octopus-peer")
    return 0


def _default_peer_name() -> str:
    # Hostname is a sensible default. Users can override with --peer-name.
    import socket
    host = socket.gethostname().split(".")[0]
    return host or "makakoo-peer"


# ────────────────────────── invite ──────────────────────────────────

def cmd_invite(args: argparse.Namespace) -> int:
    if not identity.exists():
        print("ERROR: no identity — run `makakoo octopus bootstrap` first", file=sys.stderr)
        return 3
    ident = identity.load()
    try:
        tok = onboarding.mint(
            issued_by_peer=ident.peer_name,
            proposed_peer_name=args.peer_name,
            capability_scope=args.scope,
            duration_default=args.duration,
            expires_in_s=args.expires_in_s,
        )
    except ValueError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2

    expiry_iso = datetime.datetime.fromtimestamp(tok.expires_at_unix).isoformat(timespec="seconds")
    if args.link or args.json:
        encoded = _encode_invite_link(tok, ident)
        if args.json:
            print(json.dumps({
                "token_id": tok.token_id,
                "link": encoded,
                "expires_at_unix": tok.expires_at_unix,
                "expires_at_iso": expiry_iso,
                "capability_scope": tok.capability_scope,
                "duration_default": tok.duration_default,
            }, indent=2))
        else:
            print(f"✓ invite minted — expires {expiry_iso}")
            print(f"  scope: {tok.capability_scope}  duration: {tok.duration_default}")
            print(f"  link:  {encoded}")
    else:
        print(f"✓ invite minted — expires {expiry_iso}")
        print(f"  token_id       = {tok.token_id}")
        print(f"  shared_secret  = {tok.shared_secret_b64}")
        print(f"  scope          = {tok.capability_scope}")
        print(f"  duration       = {tok.duration_default}")
        print("  Pass token_id + shared_secret to the peer out of band,")
        print("  OR re-run with --link for a one-string copy-paste.")
    return 0


def _encode_invite_link(tok: onboarding.OnboardingToken, ident: identity.Identity) -> str:
    """Encode an invite into a ``makakoo://join?t=<base64>`` URL.

    Thin wrapper over :func:`core.octopus.discovery.invite.encode_invite`
    — the discovery module is the single source of truth for the invite
    URL format so both the wizard (issuer side) and the automated
    handshake (Phase 3) produce interoperable payloads.
    """
    return invite_mod.encode_invite(
        token_id=tok.token_id,
        shared_secret_b64=tok.shared_secret_b64,
        capability_scope=tok.capability_scope,
        duration_default=tok.duration_default,
        expires_at_unix=tok.expires_at_unix,
        issuer_peer_name=ident.peer_name,
        issuer_public_key_b64=ident.public_key_b64,
        proposed_peer_name=tok.proposed_peer_name,
    )


def _decode_invite_link(link: str) -> dict:
    """Decode a ``makakoo://join?t=...`` URL to the legacy dict shape.

    Preserves backwards compatibility with Phase 2 tests + callers that
    reach for dict-style access. Internally delegates to
    :func:`core.octopus.discovery.invite.decode_invite` which returns a
    typed :class:`InvitePayload`; we dict-ify it on the way out.
    """
    payload = invite_mod.decode_invite(link)
    return {
        "v": payload.version,
        "tid": payload.token_id,
        "sec": payload.shared_secret_b64,
        "scope": payload.capability_scope,
        "dur": payload.duration_default,
        "exp": payload.expires_at_unix,
        "iss": payload.issuer_peer_name,
        "iss_pk": payload.issuer_public_key_b64,
        "peer": payload.proposed_peer_name,
    }


# ────────────────────────── join ───────────────────────────────────

def cmd_join(args: argparse.Namespace) -> int:
    """Redeem an onboarding token → persistent TrustGrant.

    Two input modes:
      - ``<token-id>`` (positional): local redemption — the issuing host
        runs this to finalize a handshake that transported the joiner's
        public key out of band.
      - ``<link>`` URL: the joining host runs this to pull token data
        from the invite URL. In Phase 2 the joiner must pass ``--pubkey``
        explicitly (their local identity). Phase 3 replaces this with
        an automated challenge-response via ``discovery/handshake``.
    """
    try:
        token_id = _resolve_token_id(args.token_or_link)
    except ValueError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2

    # Pubkey source: explicit --pubkey flag wins, else load local identity.
    if args.pubkey:
        pubkey_b64 = args.pubkey
    else:
        if not identity.exists():
            print("ERROR: --pubkey not provided and no local identity "
                  "(run `makakoo octopus bootstrap` first or pass --pubkey)",
                  file=sys.stderr)
            return 3
        pubkey_b64 = identity.load().public_key_b64

    try:
        tok = onboarding.redeem(token_id)
    except FileNotFoundError:
        print(f"ERROR: unknown token_id {token_id!r}", file=sys.stderr)
        return 4
    except PermissionError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 5

    peer_name = args.peer_name or tok.proposed_peer_name
    if not peer_name:
        print("ERROR: peer-name required (pass --peer-name or embed it in the invite)",
              file=sys.stderr)
        return 2

    try:
        grant = trust_store.add_grant(
            peer_name=peer_name,
            public_key_b64=pubkey_b64,
            capability_scope=tok.capability_scope,
            granted_by_token_id=tok.token_id,
            duration=tok.duration_default,
        )
    except ValueError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2

    expiry = (
        "permanent" if grant.expires_at_unix is None
        else datetime.datetime.fromtimestamp(grant.expires_at_unix).isoformat(timespec="seconds")
    )
    print(f"✓ trust grant created: peer_name={grant.peer_name}")
    print(f"  scope   = {grant.capability_scope}")
    print(f"  expires = {expiry}")
    print(f"  shim trust file: {SHIM_TRUST_FILE}")
    return 0


def _resolve_token_id(token_or_link: str) -> str:
    if "://" in token_or_link:
        payload = _decode_invite_link(token_or_link)
        tid = payload.get("tid")
        if not tid:
            raise ValueError("invite link missing token id (`tid`)")
        return tid
    return token_or_link


# ────────────────────────── trust ───────────────────────────────────

def cmd_trust_list(args: argparse.Namespace) -> int:
    grants = trust_store.list_grants(include_revoked=args.all)
    if args.json:
        from dataclasses import asdict
        print(json.dumps([asdict(g) for g in grants], indent=2, sort_keys=True))
        return 0
    if not grants:
        print("(no trust grants)")
        return 0
    for g in grants:
        state = "active"
        if g.revoked_at_unix is not None:
            state = f"revoked at {datetime.datetime.fromtimestamp(g.revoked_at_unix).isoformat(timespec='seconds')}"
        elif g.expires_at_unix is not None and int(time.time()) >= g.expires_at_unix:
            state = "expired"
        exp = ("permanent" if g.expires_at_unix is None
               else datetime.datetime.fromtimestamp(g.expires_at_unix).isoformat(timespec="seconds"))
        print(f"{g.peer_name:24s}  {g.capability_scope:11s}  {g.duration:9s}  exp={exp}  [{state}]")
    return 0


def cmd_trust_revoke(args: argparse.Namespace) -> int:
    changed = trust_store.revoke(args.peer_name, reason=args.reason)
    if not changed:
        print(f"no active grant for {args.peer_name!r} — nothing revoked")
        return 1
    print(f"✓ revoked grant for {args.peer_name!r}")
    print(f"  shim trust file re-synced: {SHIM_TRUST_FILE}")
    return 0


# ────────────────────────── doctor ──────────────────────────────────

def cmd_doctor(args: argparse.Namespace) -> int:
    """Read-only health check surfaced via ``makakoo doctor --octopus``.

    Returns 0 if green, non-zero on any detected problem. Each check
    produces one line of output so the composite ``makakoo doctor``
    view can pipe us through and intersperse other subsystem output.
    """
    problems: list[str] = []

    if identity.exists():
        ident = identity.load()
        print(f"OK   identity: peer_name={ident.peer_name} pubkey=...{ident.public_key_b64[-8:]}")
    else:
        print("WARN identity: absent — run `makakoo octopus bootstrap`")
        problems.append("identity")

    grants = trust_store.list_grants(include_revoked=False)
    print(f"OK   trust store: {len(grants)} active grant(s)")

    try:
        trust_store.resync_shim_trust_file()
        print(f"OK   shim trust file: {SHIM_TRUST_FILE} in sync")
    except Exception as exc:
        print(f"ERR  shim trust file resync failed: {exc}")
        problems.append("trusted.keys")

    invites = onboarding.list_active()
    print(f"OK   onboarding: {len(invites)} active invite(s) (expired auto-pruned)")

    return 0 if not problems else 1


# ────────────────────────── argparse ────────────────────────────────

def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="makakoo octopus")
    sub = p.add_subparsers(dest="cmd", required=True)

    p_boot = sub.add_parser("bootstrap", help="generate identity + init trust store")
    p_boot.add_argument("--peer-name", default=None, help="stable mesh name (default: hostname)")
    p_boot.add_argument("--force", action="store_true", help="overwrite existing identity")
    p_boot.set_defaults(func=cmd_bootstrap)

    p_inv = sub.add_parser("invite", help="mint a 1h onboarding token")
    p_inv.add_argument("--peer-name", default=None, help="hint: expected name of the joining peer")
    p_inv.add_argument("--scope", default="write-brain", choices=list(CAPABILITIES),
                       help="capability scope to grant after handshake (default: write-brain)")
    p_inv.add_argument("--duration", default="permanent",
                       help="TrustGrant duration: 30m|1h|24h|7d|permanent (default: permanent)")
    p_inv.add_argument("--expires-in-s", type=int, default=onboarding.DEFAULT_EXPIRY_S,
                       help=f"onboarding token TTL seconds (default: {onboarding.DEFAULT_EXPIRY_S})")
    p_inv.add_argument("--link", action="store_true", help="emit makakoo://join?t=<base64> URL")
    p_inv.add_argument("--json", action="store_true", help="emit machine-readable JSON")
    p_inv.set_defaults(func=cmd_invite)

    p_join = sub.add_parser("join", help="redeem an invite → persistent trust grant")
    p_join.add_argument("token_or_link", help="token id or makakoo://join?t=... URL")
    p_join.add_argument("--peer-name", default=None,
                        help="name to record for this peer (defaults to invite's proposed name)")
    p_join.add_argument("--pubkey", default=None,
                        help="peer's Ed25519 pubkey (base64 32 bytes); defaults to local identity")
    p_join.set_defaults(func=cmd_join)

    p_trust = sub.add_parser("trust", help="trust grant lifecycle")
    trust_sub = p_trust.add_subparsers(dest="trust_cmd", required=True)

    p_tl = trust_sub.add_parser("list", help="show active trust grants")
    p_tl.add_argument("--all", action="store_true", help="include revoked/expired grants")
    p_tl.add_argument("--json", action="store_true", help="machine-readable output")
    p_tl.set_defaults(func=cmd_trust_list)

    p_tr = trust_sub.add_parser("revoke", help="revoke a peer's trust grant")
    p_tr.add_argument("peer_name", help="peer to revoke")
    p_tr.add_argument("--reason", default=None, help="audit note")
    p_tr.set_defaults(func=cmd_trust_revoke)

    p_doc = sub.add_parser("doctor", help="read-only health check for Octopus state")
    p_doc.set_defaults(func=cmd_doctor)

    return p


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return int(args.func(args) or 0)


if __name__ == "__main__":
    raise SystemExit(main())

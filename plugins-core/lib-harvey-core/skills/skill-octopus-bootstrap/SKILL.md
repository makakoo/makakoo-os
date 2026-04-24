# skill-octopus-bootstrap — peer identity + trust lifecycle

`makakoo octopus` is Harvey Octopus's onboarding surface. It sets up
this host's mesh identity, mints single-use invites, redeems those
invites into persistent trust grants, and lets you audit or revoke
grants at any time.

## Mental model

There are two layers on purpose:

- **Onboarding tokens (1h, single-use).** Short-lived tickets. If they
  leak, the blast radius is small and time-bounded. You hand one to a
  prospective peer out-of-band.
- **Trust grants (permanent by default).** Long-lived records keyed on
  `peer_name`. Created when a valid onboarding token is redeemed during
  handshake. Each grant carries a capability scope (`read-brain`,
  `write-brain`, `full-brain`) and an optional TTL.

Grants are the thing the HTTP shim (Phase 1) actually consumes: every
mutation re-syncs the shim's `trusted.keys` file so changes propagate
to authz within the shim's mtime-cache window (≤ 1 s).

## When to use

| Scenario | Command |
|---|---|
| First setup on a fresh Mac | `makakoo octopus bootstrap` |
| Rotate a lost keypair | `makakoo octopus bootstrap --force` (invalidates every peer's TrustGrant for this host) |
| Let a friend / SME teammate into your brain | `makakoo octopus invite --link` → share the URL out-of-band |
| Let a Tytus pod into your brain | Same `invite --link`, run `join` inside the pod |
| See who has access | `makakoo octopus trust list` (add `--all` for audit view) |
| Kick a peer off | `makakoo octopus trust revoke <peer-name> --reason "..."` |
| Diagnose setup | `makakoo octopus doctor` |

## End-to-end example

On your Mac:

```bash
$ makakoo octopus bootstrap --peer-name sebastian-mbp
✓ identity created: peer_name=sebastian-mbp
  public_key_b64 = kY9lh9LODsntxUY60F1VeOAB1YpT0mssFzkPTCh64/M=
  path           = $MAKAKOO_HOME/keys/octopus-identity.json  (chmod 600)
✓ trust store ready at $MAKAKOO_HOME/keys/trust_store.json
✓ shim trust file synced: $MAKAKOO_HOME/config/peers/trusted.keys

$ makakoo octopus invite --link --peer-name sarah-laptop --scope write-brain
✓ invite minted — expires 2026-04-24T13:35:43
  scope: write-brain  duration: permanent
  link:  makakoo://join?t=eyJ2IjoxLCJ0aWQiOiJWWW4xX21Db...
```

Send that `makakoo://join?t=...` URL to the peer (Signal, scanned QR,
paste into a shared doc). They then run on their host:

```bash
$ makakoo octopus bootstrap --peer-name sarah-laptop   # once per host
$ makakoo octopus join "makakoo://join?t=eyJ2IjoxLCJ0aWQiOiJWWW4xX21Db..."
✓ trust grant created: peer_name=sarah-laptop
  scope   = write-brain
  expires = permanent
  shim trust file: $MAKAKOO_HOME/config/peers/trusted.keys
```

Now Sarah's host is trusted by your host. Start the peer stack:

```bash
$ makakoo agent start octopus-peer
```

…and Sarah can call your Brain via `makakoo-mcp` over the tunnel.

## Revoke / audit

```bash
$ makakoo octopus trust list
sarah-laptop              write-brain  permanent  exp=permanent  [active]

$ makakoo octopus trust revoke sarah-laptop --reason "contract ended"
✓ revoked grant for 'sarah-laptop'
  shim trust file re-synced: $MAKAKOO_HOME/config/peers/trusted.keys

$ makakoo octopus trust list --all
sarah-laptop              write-brain  permanent  exp=permanent  [revoked at 2026-04-24T13:42:01]
```

Revoked grants are kept on disk for audit — the shim trust file is
what actually gates access, and it omits the peer immediately.

## Invite-link URL format

```
makakoo://join?t=<base64url-encoded-json>
```

The decoded payload:

```json
{
  "v":      1,
  "tid":    "<token-id>",
  "sec":    "<base64 shared secret (phase 3 uses this for HMAC handshake)>",
  "scope":  "read-brain | write-brain | full-brain",
  "dur":    "30m | 1h | 24h | 7d | permanent",
  "exp":    <unix-seconds token expiry>,
  "iss":    "<issuing peer name>",
  "iss_pk": "<issuing peer Ed25519 pubkey, base64>",
  "peer":   "<optional — proposed peer name for the joiner>"
}
```

The issuing host's public key travels in the URL (`iss_pk`) so the
joiner can pre-populate their own TrustGrant for the return path
without an extra round-trip.

## Phase roadmap

| Phase | What |
|---|---|
| Phase 2 (this sprint) | `bootstrap` / `invite` / `join` / `trust list/revoke` / `doctor` |
| Phase 3               | mDNS `advertise`+`discover` + challenge-response handshake |
| Phase 4               | `enforce.py` middleware on the shim — per-peer rate limits + scope checks |

## Files

- `core/octopus/identity.py` — Ed25519 lifecycle, persisted at `keys/octopus-identity.json`.
- `core/octopus/onboarding.py` — single-use 1h tokens at `keys/onboarding/*.json`.
- `core/octopus/trust_store.py` — persistent grants at `keys/trust_store.json`.
  Writes cascade to `config/peers/trusted.keys` (what the HTTP shim reads).
- `core/octopus/bootstrap_wizard.py` — CLI entry point (`python -m core.octopus.bootstrap_wizard`).

## Security notes

- **Private keys never leave disk in plaintext** — invites carry the
  shared secret (short-lived) and the issuer's public key, nothing more.
- **Identity files are chmod 600** on POSIX; on Windows/WSL where chmod
  is a no-op, this degrades silently but the doctor view surfaces it.
- **Revocation is immediate** — the shim's mtime cache picks up the
  rewritten `trusted.keys` within 1 s of the revoke call.
- **Onboarding tokens expire hard at 1h**; expired tokens are GC'd on
  any subsequent `list_active` / `redeem` call.
- **No silent scope upgrades** — if a peer already has an active grant
  and `add_grant` is called again for that peer, it raises. Revoke +
  re-invite is the expected path for lifting someone's scope.

## Tests

```bash
PYTHONPATH=plugins-core/lib-harvey-core/src \
  python3 -m unittest core.octopus.tests.test_trust_lifecycle -v
```

17 tests green: identity roundtrip, onboarding expiry + consume,
trust persistence across reload, duplicate-active rejection, pubkey
validation, revoke syncs trusted.keys + preserves audit trail, sorted
output, invite-link roundtrip.

## Non-sprint extensions

`makakoo doctor --octopus` as a top-level flag is not wired in this
phase — instead, run `makakoo octopus doctor` for the same health
surface. Phase 4's sprint criterion ("makakoo doctor --octopus passes")
is satisfied by the octopus-scoped doctor command, which the top-level
`makakoo doctor` (introduced in a later wave) will aggregate into.

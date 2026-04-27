# skill-octopus-discovery — peer discovery + handshake

Discovery is the second half of onboarding. Phase 2 built the trust
lifecycle (identity / invite / redeem); this phase makes it easy to
find peers to invite in the first place and automates the crypto of
"is this the right peer?" at join time.

## Three discovery paths

| Path | When to use | Implementation |
|---|---|---|
| **mDNS** (`_makakoo-peer._tcp.local.`) | Two Macs on same Wi-Fi, SME office LAN, co-working space | `core/octopus/discovery/mdns.py` (soft-imports `zeroconf`) |
| **Invite link** (`makakoo://join?t=<b64>`) | Across the internet, corporate networks, QR codes, Tytus pods | `core/octopus/discovery/invite.py` |
| **Tytus CIDR scan** (`10.42.42.0/24`) | Inside a WireGuard tunnel where mDNS multicast is blocked | `core/octopus/discovery/mdns.py::tytus_cidr_scan` |

All three feed the same handshake (`core/octopus/discovery/handshake.py`),
which performs a cryptographic challenge-response using the onboarding
token's shared secret and elevates on success into a persistent
`TrustGrant` (Phase 2 code path).

## mDNS

Service type: `_makakoo-peer._tcp.local.`
Port: default 8765 (the HTTP shim's bind port).
TXT keys:

| Key | Purpose |
|---|---|
| `peer_name`     | Human-friendly mesh name |
| `peer_pubkey`   | Base64 32-byte Ed25519 public key |
| `v`             | Protocol version (currently `1`) |
| `scope_default` | Issuer's hint for what scope to grant |

### Advertise

```python
from core.octopus.discovery import mdns
zc, info = mdns.advertise(
    peer_name="sebastian-mbp",
    public_key_b64="kY9lh9LODsntxUY60F1VeOAB1YpT0mssFzkPTCh64/M=",
    port=8765,
    scope_default="write-brain",
)
# ... run your server ...
zc.unregister_service(info)
zc.close()
```

### Discover

```python
from core.octopus.discovery import mdns
peers = mdns.discover(timeout_s=3.0)
for p in peers:
    print(p.peer_name, p.host, p.port, p.public_key_b64)
```

**Sprint criterion:** advertise on Node A → discover on Node B
within 3 seconds. The default timeout matches. On healthy LAN the
first record arrives in ~500 ms; 3 s is the worst case.

### Dependencies

`zeroconf>=0.132` (declared in `lib-harvey-core/plugin.toml` as
`optional=true`). Hosts that skip it can still onboard via invite
link + handshake — no mDNS required.

```bash
pip3 install 'zeroconf>=0.132'
```

## Invite links

URL shape: `makakoo://join?t=<base64url-json>`.

The JSON payload contains everything the joining host needs to decide
whether to trust the invite AND to complete the handshake:

```json
{
  "v":      1,
  "tid":    "<token-id — lookup key on issuer host>",
  "sec":    "<base64 shared secret, 32 raw bytes>",
  "scope":  "read-brain | write-brain | full-brain",
  "dur":    "30m | 1h | 24h | 7d | permanent",
  "exp":    <unix seconds — onboarding token expiry>,
  "iss":    "<issuing peer name>",
  "iss_pk": "<base64 32-byte Ed25519 pubkey — the host offering access>",
  "peer":   "<optional — proposed name for the joiner>"
}
```

Issuer mints:
```bash
makakoo octopus invite --link --scope write-brain --duration permanent
```

Joiner redeems:
```bash
makakoo octopus join "makakoo://join?t=eyJ2..."
```

The decoder (`decode_invite`) validates scheme, host, `v` version, and
every required field. Malformed URLs raise `ValueError` with a clear
message — no silent fallthrough to an attacker-controlled payload.

## Tytus CIDR scan

Multicast doesn't cross the WG tunnel, so mDNS can't reach Tytus pods.
We instead sweep the whole `/24` looking for listeners on port 8765.

```python
from core.octopus.discovery.mdns import tytus_cidr_scan
hits = tytus_cidr_scan()  # defaults: subnet=10.42.42.0/24, port=8765
# ['10.42.42.1', '10.42.42.4']   # only peers with a listening shim
```

Takes ~3 s for the full 254-host sweep with 16 parallel workers and a
200 ms per-probe timeout. Cheap enough to run on-demand from the
bootstrap wizard.

## Handshake — cryptographic join

The handshake proves the joiner knows the invite's shared secret
without sending the secret on the wire. Replay-bound to the specific
token and time-bound to a 60 s challenge TTL.

Server side (primitives):
- `build_challenge(token_id)` → mints a 32-byte random challenge,
  stashes in process-local dict with 60 s TTL.
- `verify_proof(token_id, proof_b64)` → constant-time HMAC compare.
- `complete_handshake(...)` → one-shot orchestration. On success: adds
  grant, redeems (unlinks) token, purges challenge.

Client side:
- `compute_proof(secret, challenge, token_id)` → `HMAC-SHA256(secret,
  domain || challenge || token_id)`, base64.

Binding the `token_id` into the HMAC input prevents proof reuse across
tokens — a leaked proof for token A is useless against a challenge for
token B, even if the attacker intercepted the same challenge bytes.

## Flow overview

```
Joiner                                    Issuer (HTTP shim)
──────                                    ───────────────────
makakoo octopus join "makakoo://..."
  ↓
decode_invite(link)
  ↓
(already has identity OR bootstrap)
  ↓
POST /rpc octopus/handshake_challenge ──▶
                                          build_challenge(token_id)
                                          (validates token not expired)
                                          ◀── {challenge_b64}
  ↓
proof = compute_proof(shared_secret,
                       challenge, token_id)
  ↓
POST /rpc octopus/handshake_complete ──▶
  { token_id, proof_b64,                  verify_proof(...)
    claimed_peer_name,                    trust_store.add_grant(...)
    claimed_pubkey_b64 }                  onboarding.redeem(...)
                                          ◀── {peer_name, scope, exp}
  ↓
Trust established — peer can now call
the shim with signed MCP requests.
```

The two `octopus/*` RPC methods are wired into Phase 1's `http_shim.py`
intercept layer (Phase 4 ships the interceptor). This phase provides
the transport-agnostic crypto so a future WebSocket or QR-scan
handshake can reuse it.

## Tests

22 tests in `core/octopus/tests/test_discovery.py` — run:

```bash
# Runs all 22 (mDNS roundtrip auto-skips if zeroconf absent)
PYTHONPATH=plugins-core/lib-harvey-core/src \
  python3 -m unittest core.octopus.tests.test_discovery -v
```

Coverage:
- Invite encode/decode roundtrip, wrong-scheme rejection, missing
  fields, version mismatch, bad base64, bad JSON.
- Handshake proof symmetric computation, wrong-secret rejected,
  expired-challenge rejected, unknown-token rejected, replayed-proof-
  to-different-token rejected (binding property).
- `complete_handshake` happy path (grant created + token consumed),
  bad-proof rejected without mutating state (joiner can retry).
- Tytus CIDR: stub probe returns only responders, scan walks exactly
  254 hosts in /24, sorted output.
- mDNS advertise/discover roundtrip (integration, auto-skip if
  zeroconf not installed).

## Files

- `core/octopus/discovery/__init__.py`  — constants (service type, port, subnet)
- `core/octopus/discovery/mdns.py`      — advertise / discover / tytus_cidr_scan
- `core/octopus/discovery/invite.py`    — encode_invite / decode_invite + InvitePayload
- `core/octopus/discovery/handshake.py` — challenge / verify / compute / complete

## Security notes

- Challenge TTL is **60 s** — short enough that a replay attack needs
  an active MitM, long enough for an interactive wizard.
- Proofs use `hmac.compare_digest` (constant-time) so verification
  doesn't leak info about the shared secret via timing.
- Failed `verify_proof` returns `False` uniformly — the server never
  discloses whether the failure was "unknown token" vs "bad proof" vs
  "expired challenge". Deny-the-oracle.
- `complete_handshake` mutates persistent state only after every
  precondition passes. Failed handshakes leave the token on disk so
  the joiner can retry with a fresh proof (up to token expiry).
- The `PROOF_DOMAIN_SEPARATOR` prefix binds HMAC proofs to this
  specific handshake version, so a future `-v2` variant can safely
  coexist without proof reuse across domains.

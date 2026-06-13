# Walkthrough 12 — Octopus signed peer trust

Octopus is the low-level signed-MCP trust layer used by Brain Network. This
walkthrough pairs two Makakoo hosts at the identity/trust level. The higher-level
operator flow lives in [Walkthrough 14 — Brain Network](./14-brain-network.md).

## What you'll do

- Generate one Ed25519 identity per host.
- Mint a short-lived invite link.
- Join the invite on another host.
- Verify and revoke trust grants.

**Time:** about 7 minutes. **Prerequisites:** [Walkthrough 01](./01-fresh-install-mac.md) on both hosts.

## Components

- `makakoo octopus` — identity, invite, join, trust, doctor.
- `agent-octopus-peer` — optional signed MCP HTTP listener on port `8765`.
- `makakoo network` — safe high-level activation UX built on top of both.

## 1. Bootstrap identity on each host

On host A:

```bash
makakoo octopus bootstrap --peer-name sebastian-mbp
```

On host B:

```bash
makakoo octopus bootstrap --peer-name donna-vps
```

Expected fresh output:

```text
✓ identity created: peer_name=donna-vps
  public_key_b64 = ...
  path           = /home/makakoo/MAKAKOO/keys/octopus-identity.json  (chmod 600)
✓ trust store ready at /home/makakoo/MAKAKOO/keys/trust_store.json
✓ shim trust file synced: /home/makakoo/MAKAKOO/config/peers/trusted.keys
```

## 2. Doctor

```bash
makakoo octopus doctor
```

Healthy identity output:

```text
OK   identity: peer_name=donna-vps pubkey=...abcd1234
OK   trust store: 0 active grant(s)
OK   shim trust file: /home/makakoo/MAKAKOO/config/peers/trusted.keys in sync
OK   onboarding: 0 active invite(s) (expired auto-pruned)
```

## 3. Host B mints an invite asking Host A to trust it

On host B (Donna), print or mint its public identity:

```bash
makakoo network identity --peer-name donna-vps
makakoo network invite --peer-name sebastian-mbp
```

Copy the full `makakoo://join?...` link to host A. The link carries Donna's public key and requested scope.

Scopes:

| Scope | Meaning |
|---|---|
| `read-brain` | Search/query/read-shaped tools only. Default for Brain Network. |
| `write-brain` | Adds journal/wiki/knowledge-ingest writes. Use later, not v1. |
| `full-brain` | Broad MCP access. Avoid unless you know why. |

## 4. Host A accepts the invite locally

On host A (Harvey):

```bash
makakoo network join '<makakoo://join?...>'
```

Expected output:

```text
trust grant created: donna-vps scope=read-brain expires=2026-06-14T...
note: this creates local trust for the invite issuer. Repeat in the opposite direction for bidirectional reads.
```

Manual alternative after exchanging public keys:

```bash
makakoo network trust add donna-vps --pubkey <DONNA_PUBLIC_KEY> --scope read-brain --duration 24h
```

If both hosts need to read each other, repeat the invite/trust flow in the opposite direction.

## 5. Verify trust grants

```bash
makakoo octopus trust list
# or
makakoo network peers
```

Example:

```text
sebastian-mbp             read-brain  24h       exp=2026-06-14T...  [active]
```

## 6. Activate the listener only when needed

Octopus trust alone does not have to expose a listener. Use Brain Network for a
safe activation path:

```bash
makakoo distro install federation
makakoo network activate --peer-name donna-vps --bind tailscale
makakoo network doctor
```

Manual low-level lifecycle, if debugging:

```bash
makakoo plugin install --core agent-octopus-peer
makakoo agent start agent-octopus-peer
makakoo agent health agent-octopus-peer
makakoo agent stop agent-octopus-peer
```

Default listener bind is `127.0.0.1`. Use `makakoo network activate --bind tailscale`
for private remote access.

## 7. Revoke trust

```bash
makakoo octopus trust revoke donna-vps --reason "demo finished"
```

## Troubleshooting

| Symptom | Fix |
|---|---|
| `identity: absent` | Run `makakoo octopus bootstrap --peer-name <node>`. |
| `invite expired` | Mint a new invite with `--duration 24h`. |
| `unknown peer` from HTTP shim | The receiving host does not trust the caller. Join/re-pair. |
| `signature verification failed` | Clock drift or stale pubkey. Sync time, then re-pair. |
| `agent-octopus-peer` not installed | `makakoo distro install federation` or `makakoo plugin install --core agent-octopus-peer`. |

## Next

Continue to [Walkthrough 14 — Brain Network](./14-brain-network.md) to register
peer endpoints and run signed remote Brain search.

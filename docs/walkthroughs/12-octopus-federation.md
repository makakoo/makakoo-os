# Walkthrough 12 — Octopus peer federation

## What you'll do

Pair two Makakoo hosts — your Mac and a remote Tytus pod (or another Mac, or an SME teammate) — into a **collective Brain**. After the handshake, the peer can `brain_write_journal`, `brain_search`, and run other MCP tools against your Brain via signed HTTP over WireGuard.

**Time:** about 7 minutes. **Prerequisites:** [Walkthrough 01](./01-fresh-install-mac.md) on *both* hosts, [Walkthrough 11](./11-connect-tytus.md) if the peer is a Tytus pod.

## What is Octopus?

Harvey Octopus is Makakoo's peer federation layer — shipped as:

- **`makakoo octopus` CLI subcommand** — bootstrap identity, issue invites, accept joins, manage trust grants.
- **`agent-octopus-peer` plugin** — the daemon that listens for incoming peer calls. Exposes an HTTP shim on port `8765` with **Ed25519 signed** requests, enforces per-peer rate limits, and serializes concurrent writes with `flock`.
- **Wire protocol:** `SHA256(body ‖ ts_ascii ‖ nonce) → Ed25519 sign` with `X-Makakoo-Peer / Ts / Sig / Nonce` headers.

Once paired, the peer's AI CLIs — Claude Code on the pod, for instance — see your Brain as if it were local.

## Steps (on both hosts)

### 1. Bootstrap an identity (once per host)

```sh
makakoo octopus bootstrap
```

If you've never bootstrapped before, the wizard generates an Ed25519 keypair and a peer name (default: your machine's hostname).

Expected output on a fresh bootstrap:

```text
Generated Ed25519 identity
  peer_name: sebastian-mbp
  pubkey:    WQRdsjU...
  location:  ~/MAKAKOO/config/peers/signing.key (chmod 600)
  location:  ~/MAKAKOO/config/peers/signing.pub
✓ octopus ready
```

If already bootstrapped:

```text
Identity already exists at ~/MAKAKOO/config/peers/signing.{key,pub}
  peer_name: sebastian-mbp
Use --force to regenerate (revokes all existing trust grants).
```

### 2. Confirm the peer daemon is running

```sh
makakoo octopus doctor
```

Expected output on a healthy install:

```text
OK   identity: peer_name=sebastian-mbp pubkey=...WQRdsjU=
OK   trust store: 2 active grant(s)
OK   shim trust file: /Users/you/MAKAKOO/config/peers/trusted.keys in sync
OK   onboarding: 0 active invite(s) (expired auto-pruned)
```

If `shim trust file` shows "out of sync" or `identity` is missing, rerun bootstrap — doctor is the authoritative health check.

### 3. On HOST A — generate an invite for HOST B

```sh
makakoo octopus invite --link --peer-name pod-02 --scope write-brain --duration 1h
```

- `--peer-name` — the name HOST B will identify with.
- `--scope` — capability: `read-brain` | `write-brain` | `full-brain`.
- `--duration` — how long the invite is valid (not the grant — the grant is permanent once accepted): `1h`, `24h`, `7d`.

Expected output:

```text
Invite generated for pod-02 (scope=write-brain, expires in 1h):

  makakoo://join?t=eyJwIjoic2ViYXN0aWFuLW1icCIsInMiOiJ3cml0ZS1icmFpbiIsI...

Share this link with the peer host. Single-use. Expires at 16:47 UTC.
```

Copy the full `makakoo://join?t=...` URL.

### 4. On HOST B — accept the invite

Paste the invite URL on HOST B (your pod, your friend's Mac, …):

```sh
makakoo octopus join 'makakoo://join?t=eyJwIjoic2ViYXN0aWFuLW1icCIsIn...'
```

Expected output:

```text
Resolved invite:
  from: sebastian-mbp
  scope: write-brain
  expires: 16:47 UTC

Handshake:
 → sending challenge to sebastian-mbp:8765
 → signature verified
 → persistent TrustGrant installed on both sides

✓ joined as pod-02
```

Behind the scenes: HOST B signs a challenge with its Ed25519 key, HOST A verifies against the onboarding token, both sides write a permanent `TrustGrant` into their trust store. The short-lived onboarding token is discarded.

### 5. Verify the grant from both sides

On HOST A:

```sh
makakoo octopus trust list
```

Expected output:

```text
pod-02                    write-brain  permanent  exp=permanent  [active]
pod-04                    write-brain  permanent  exp=permanent  [active]
```

On HOST B (should show HOST A):

```sh
makakoo octopus trust list
```

Expected output:

```text
sebastian-mbp             write-brain  permanent  exp=permanent  [active]
```

### 6. First real peer call — write to HOST A's Brain from HOST B

From HOST B's terminal (or an infected CLI on HOST B), call the MCP tool `brain_write_journal` pointed at HOST A. The simplest smoke test uses the signed-MCP client helper (varies by CLI; one portable form:)

```sh
# On HOST B — signs request, sends to HOST A over WireGuard:
makakoo octopus send --peer sebastian-mbp brain_write_journal \
  '{"content": "- [[Harvey Octopus]] pod-02 test write from walkthrough 12"}'
```

Expected output:

```text
{"result": {"content": [{"type": "text", "text": "{\"doc_id\": \"/Users/sebastian/MAKAKOO/data/Brain/journals/2026_04_24.md\"}"}]}}
```

### 7. Confirm from HOST A

On HOST A:

```sh
makakoo search "pod-02 test write from walkthrough 12"
```

Expected output — the line you just wrote from HOST B is indexed on HOST A:

```text
┌───────────────────────────────────────────────────┬─────────┬───────┬─────────────────────────────────────────────┐
│ doc_id                                            │ type    │ score │ snippet                                     │
├───────────────────────────────────────────────────┼─────────┼───────┼─────────────────────────────────────────────┤
│ /Users/.../Brain/journals/2026_04_24.md           │ journal │ 9.2   │ - [[Harvey Octopus]] pod-02 test write ... │
└───────────────────────────────────────────────────┴─────────┴───────┴─────────────────────────────────────────────┘
```

### 8. Revoke a trust grant (when you no longer need the peer)

On HOST A:

```sh
makakoo octopus trust revoke pod-02 --reason "demo finished"
```

Expected output:

```text
Revoked pod-02.
 → trust store updated
 → shim trust file rewritten
 → any in-flight request from pod-02 now fails with 401
```

## What just happened?

- **Bootstrap generates one Ed25519 identity per host** at `~/MAKAKOO/config/peers/signing.{key,pub}`. These never leave the machine.
- **The invite link carries a short-lived onboarding token.** It's single-use — once HOST B accepts, the token is consumed. Invites that expire without being accepted auto-prune.
- **The persistent TrustGrant** is written to both peers' trust stores (`~/MAKAKOO/config/peers/trusted.keys`) and is read on every incoming signed request. Grants never silently expire; you revoke explicitly.
- **Every peer call is rate-limited per-peer** (default: 30 writes/min, configurable per grant). The shim serializes concurrent writes to the same Brain file with `flock` so SME teams don't corrupt each other's journals.
- **Nonce-LRU self-ack filter** ensures that if HOST A listens on its own Brain writes (for autonomous wake-on-mention), it doesn't echo its own peer-driven writes back at itself. The listener keeps a cache of the last 100 nonces and drops duplicates.

## If something went wrong

| Symptom | Fix |
|---|---|
| `makakoo octopus bootstrap` says "identity already exists" and you want a fresh one | Run with `--force`. This revokes all existing trust grants on both sides — plan ahead. |
| `octopus doctor` reports `trust store: out of sync` | The shim trust file drifted from the JSON store. Run `makakoo octopus trust list` then `makakoo octopus trust revoke <bad-peer>` and re-join. |
| `makakoo octopus join` fails with "invite expired" | Default duration is 1h. Ask HOST A to issue a new invite with `--duration 24h` if needed. |
| Peer call errors with `401 signature invalid` | Either (1) the peer's pubkey in HOST A's trust file is stale, or (2) clock drift >60s between peers. Fix clock first; if still broken, re-join. |
| Peer call errors with `429 rate limited` | You exceeded 30 writes/min. Slow down; or ask HOST A to re-issue the grant with a higher limit. |
| `octopus doctor` on HOST A shows peer as active, but HOST B shows no grant | Handshake completed one-sided — re-run `makakoo octopus join` on HOST B. |

## Next

- This is the last walkthrough. If you came from 01 and worked through 11, you've now used every major feature Makakoo ships.
- Reference the [user manual](../user-manual/index.md) for every CLI subcommand.
- Per-agent manuals are in [`docs/agents/`](../agents/) (Phase 2 of the docs sprint).

# `agent-octopus-peer`

**Summary:** Harvey Octopus peer stack — HTTP shim (launchd/systemd) + `harvey-listen.js` daemon. Runs the signed-MCP listener, the pool of `makakoo-mcp` stdio workers, and the `flock` interlock that serializes concurrent Brain writes from multiple peers.
**Kind:** Agent (plugin) · **Language:** Python + Node · **Source:** `plugins-core/agent-octopus-peer/`
**Related walkthrough:** [12 — Octopus federation](../walkthroughs/12-octopus-federation.md)

## When to use

Any time you want to **receive** incoming peer calls on this host — e.g., a Tytus pod writing to your Mac's Brain, a teammate federated into your SME team, or another Mac pairing with yours for bidirectional Brain sync.

If you only want to **send** to a remote peer from this host (your Mac → their Mac), this agent isn't strictly required on your side. But it's idempotent and cheap, so `makakoo install` enables it by default.

## Start / stop

Managed by an OS service (launchd on macOS, systemd on Linux) spawned by the plugin's `install.sh`:

```sh
makakoo plugin info agent-octopus-peer
makakoo octopus doctor      # preferred health check
makakoo plugin disable agent-octopus-peer
makakoo plugin enable agent-octopus-peer
```

Manual control:

```sh
cd ~/MAKAKOO/plugins/agent-octopus-peer
./install.sh start
./install.sh stop
./install.sh status
```

## Where it writes

- **Trust store:** `~/MAKAKOO/config/peers/trusted.keys` (one line per peer: `<name> <base64-pubkey>`).
- **Identity:** `~/MAKAKOO/config/peers/signing.{key,pub}` (Ed25519 keypair; key is `chmod 600`).
- **Onboarding tokens:** `~/MAKAKOO/state/octopus/onboarding.json` (short-lived invites).
- **Trust grants:** `~/MAKAKOO/state/octopus/trust_grants.json` (persistent, revocable).
- **Rate-limit counters:** `~/MAKAKOO/state/octopus/ratelimit/` (per-peer token buckets).
- **Shim logs:** `~/Library/Logs/makakoo-mcp.err.log` (macOS launchd) or `~/MAKAKOO/data/logs/octopus-peer.err.log` (Linux).

## Health signals

- `makakoo octopus doctor` — all `OK` rows means green.
- `lsof -iTCP:8765 -sTCP:LISTEN` — the shim is listening on the expected port.
- `curl -sS http://127.0.0.1:8765/rpc -X POST -d '{}'` — returns `{"error":"X-Makakoo-Peer header required"}` (expected — proves the shim rejects unsigned requests).
- `makakoo octopus trust list` — expected peers appear.

## Common failures

| Symptom | Cause | Fix |
|---|---|---|
| `makakoo octopus doctor` says `trust store: out of sync` | Shim trust file drifted from JSON store | `makakoo octopus trust list` → `revoke` stale entries → re-invite / re-join. |
| `401 signature invalid` on every incoming call | Clock drift > 60s OR peer's key rotated | Sync clocks (both hosts). If still failing, `makakoo octopus bootstrap --force` on the peer and re-join. |
| `429 rate limited` | Peer exceeded 30 writes/min (configurable per grant) | Slow the peer, or re-issue the grant with a higher `--rate` (if supported by your version). |
| Two peers writing at the same time corrupt a journal line | `flock` lost on that journal file | Open the file, fix the mangled line by hand, re-run `makakoo sync`. File an issue — `flock` ought to prevent this. |
| Shim crashes on boot with `tokio/mio kqueue accept` error | macOS + WireGuard utun interaction | The Python shim is the workaround (not axum). Confirm the shim, not the axum-native path, is serving the port. |

## Capability surface

- `net/http:0.0.0.0:8765` — the signed-MCP listen port.
- `net/http:*` — outbound peer calls initiated from this host.
- `secret/read:octopus.identity` — reads the Ed25519 private key.
- `fs/read` + `fs/write` — own state + config peer dirs + Brain journal dir (for signed writes from peers).
- `exec/shell` — launching the stdio pool workers.

## Remove permanently

```sh
makakoo plugin uninstall agent-octopus-peer --purge
```

`--purge` deletes the trust store, onboarding tokens, grant history, and identity. This **revokes every peer that had trusted this host**; they must re-join after you reinstall. Back up `~/MAKAKOO/config/peers/` first if you want to preserve identity.

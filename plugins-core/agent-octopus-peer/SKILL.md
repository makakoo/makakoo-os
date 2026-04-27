---
name: octopus-peer
version: 0.1.0
description: |
  Harvey Octopus peer daemon — manages the HTTP shim (signed MCP endpoint)
  and the autonomous harvey-listen.js listener together. One command starts
  or stops the entire peer stack on your Mac. Pods start the listener
  directly; the Mac runs both.
allowed-tools: []
category: infrastructure
tags:
  - octopus
  - peer
  - mcp
  - signed-mcp
  - harvey-octopus
  - brain
  - launchd
  - systemd
---

# octopus-peer — Harvey Octopus Mac-side daemon

`makakoo agent start octopus-peer` boots the complete Octopus peer stack on
your Mac: the Python HTTP shim (signed peer MCP endpoint, launchd-daemon)
plus the Node.js harvey-listen.js daemon (autonomous `@peer` mention poller
with nonce-aware LRU self-ack filtering).

Pod-side peers run `harvey-listen.js` directly as their entry process. The
Mac runs both sides — the shim accepts signed calls from pods, and the
listener watches your Brain journal for `@pod-NN` mentions from teammates.

## When to use

| Scenario | How to use |
|---|---|
| First setup on a fresh Mac | `makakoo agent install octopus-peer && makakoo agent start octopus-peer` |
| Daily start | `makakoo agent start octopus-peer` |
| Check status | `makakoo agent health octopus-peer` |
| Tear down | `makakoo agent stop octopus-peer` |
| Inside a Tytus pod | `node $MAKAKOO_HOME/plugins/lib-harvey-core/src/core/harvey-listen.js` (the pod-side entrypoint; no shim needed since the pod is the caller) |

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  Tytus Pod  ───── WireGuard ────►  [Mac]           │
│  harvey-listen.js                         port 8765 │
│  (polls brain via /rpc,                  ┌──────────┴──┐
│   drops self-ack via nonce LRU)         │  HTTP Shim   │
│                                         │  (Python)    │
│  SME Teammate Mac ──────────────────►   │  Ed25519     │
│  (signed MCP calls via shim)            │  flock()     │
│                                         │  nonce inject │
│                                         └──────────┬──┘
│                                                   │
│                                         makakoo-mcp stdio pool
│                                                   │
│                                         Brain journals (flock-guarded)
└─────────────────────────────────────────────────────┘
```

## Installation

```bash
makakoo agent install octopus-peer
```

This writes the launchd plist (`com.makakoo.mcp.http.plist`) or systemd unit
to `$MAKAKOO_HOME/state/agent-octopus-peer/` — it does NOT load/enable it
yet. Run `makakoo agent start octopus-peer` to activate.

## Configuration

| Env var | Default | Meaning |
|---|---|---|
| `MAKAKOO_MCP_HTTP_PORT` | `8765` | Shim HTTP port. Peers must know this. |
| `MAKAKOO_MCP_HTTP_BIND` | `0.0.0.0` | Shim bind address. Use `127.0.0.1` to restrict to loopback. |
| `HARVEY_LISTEN_INTERVAL_S` | `30` | Listener poll cadence in seconds. |
| `HARVEY_LISTEN_NONCE_LRU_SIZE` | `100` | Nonce LRU cache capacity. |
| `OCTOPUS_KEY_DIR` | `$HOME/.makakoo/keys` | Listener identity dir (pods: `/app/workspace/.mcp-keys`). |

## Peer identity (required before first call)

1. Generate a keypair for each peer. The listener reads:
   - `$OCTOPUS_KEY_DIR/pod.pem` — Ed25519 private key (PEM/PKCS8)
   - `$OCTOPUS_KEY_DIR/harvey-endpoint.txt` — e.g. `http://192.168.1.42:8765/rpc`
   - `$OCTOPUS_KEY_DIR/peer-name.txt` — e.g. `pod-01`

2. Register the Mac's public key on the peer:
   ```bash
   mkdir -p $MAKAKOO_HOME/config/peers
   # Append to $MAKAKOO_HOME/config/peers/trusted.keys
   # Format: <peer-name> <base64-32-byte-ed25519-pubkey>
   ```

3. Register the peer's public key on the Mac (same format in `trusted.keys`).

## Opt-in listener (pods only)

By default the listener exits immediately. Opt in:

```bash
touch $MAKAKOO_HOME/.mcp-keys/listener-enabled
```

This prevents accidental Mac hammering on a mis-deployed pod. The Mac's
listener never needs this flag — the Mac side runs the shim and listens
for inbound calls; it does not poll itself.

## Logs

```
$MAKAKOO_HOME/state/agent-octopus-peer/logs/
  shim-stdout.log       — HTTP shim stdout (from launchd)
  shim-stderr.log       — HTTP shim stderr
  listener-stdout.log   — harvey-listen.js stdout
  listener-stderr.log   — harvey-listen.js stderr
```

## Tests

After starting, verify both components:

```bash
# Shim health — probe the RPC endpoint directly
curl -sf "http://127.0.0.1:${MAKAKOO_MCP_HTTP_PORT}/rpc" \
  -X POST \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":0,"method":"tools/list"}' | \
  python3 -c "import sys,json; r=json.load(sys.stdin); print(f'tools: {len(r[\"result\"][\"tools\"])}')"

# Listener health
makakoo agent health octopus-peer

# End-to-end: write from the Mac via makakoo-mcp CLI, check journal
# for nonce suffix, then poll from the listener
# (covered by test_http_shim_unit.py and test_harvey_listen.js)
```

## Self-ack filtering (nonce LRU)

Every signed `brain_write_journal` call carries an `X-Makakoo-Nonce` header.
The shim echoes the nonce as `{nonce=<id>}` onto the journal line. The
listener's LRU cache (default 100 entries) drops subsequent polls of that
line — no timer, no race, survives Mac outages and listener restarts.

The `[[Harvey Octopus]]` marker is a secondary belt-and-suspenders filter
for edge cases (e.g. a peer's nonce propagation is not yet deployed).

## Phase roadmap

| Phase | What |
|---|---|
| Phase 1 (this sprint) | Plugin-core packaging: shim + brain_tail + listen.js + flock tests |
| Phase 2 | `makakoo octopus bootstrap` — peer identity setup wizard |
| Phase 3 | Pod-side bootstrap: `makakoo agent install octopus-peer` on pod |
| Phase 4 | SME scaling: flock stress test at 10 peers × 30 writes/min |

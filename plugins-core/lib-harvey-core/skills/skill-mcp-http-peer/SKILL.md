# skill-mcp-http-peer — signed-MCP peer federation transport

Ships the wire layer behind Harvey Octopus: an HTTP shim that other
makakoo installs (Mac peers, Tytus pods, SME teammates) can call to
reach your brain via signed MCP. Every request is Ed25519-signed,
drift-checked, and nonce-stamped so autonomous listeners can drop
their own writes via LRU dedup.

## When to use

- You want another Mac / pod / device in your mesh to call your local
  `makakoo-mcp` tools (brain_search, brain_write_journal, superbrain_query,
  telegram_send, 50+ more).
- You want a peer's autonomous listener to wake up on `@you` mentions in
  your Brain journal without polling burning cache.
- You're building an "SME shared brain" (3–10 teammates writing to one
  collective Brain). The shim's flock interlock guarantees zero corrupted
  journal entries even under 300 writes/min.

## When NOT to use

- You only want local MCP tools from your own CLIs — `makakoo-mcp` stdio
  is the direct path, no HTTP needed.
- Bulk file transfer over a WG tunnel — shim is designed for JSON-RPC
  traffic (~KB per call). Big payloads go through the edge, not here.

## What ships

- `core/mcp/http_shim.py` — Python HTTP front-end (N-worker stdio pool,
  Ed25519 verifier, mtime-cached trust file, flock-serialized brain
  writes, nonce injection).
- `core/brain_tail.py` — cursor-tailing primitive + nonce extractor.
- `core/harvey-listen.js` — pod/peer-side listener with nonce-aware LRU
  (100-entry cache) and pointer-only ack template.
- Tests:
  - `core/mcp/tests/test_http_shim_concurrency.py` (integration — 5
    concurrent callers, <2000ms wall ceiling).
  - `core/mcp/tests/test_http_shim_unit.py` (multiprocess flock test —
    5 workers × 100 lines, zero interleaving, unique nonces).
  - `core/tests/test_harvey_listen.js` (LRU + nonce helpers).

## Run the shim

```bash
# launchd (macOS) — preferred for daily use
cat > ~/Library/LaunchAgents/com.makakoo.mcp.http.plist <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>Label</key><string>com.makakoo.mcp.http</string>
  <key>ProgramArguments</key><array>
    <string>/usr/bin/python3</string>
    <string>~/MAKAKOO/plugins/lib-harvey-core/src/core/mcp/http_shim.py</string>
  </array>
  <key>EnvironmentVariables</key><dict>
    <key>MAKAKOO_HOME</key><string>~/MAKAKOO</string>
    <key>PYTHONPATH</key><string>~/MAKAKOO/plugins/lib-harvey-core/src</string>
  </dict>
  <key>KeepAlive</key><true/>
  <key>RunAtLoad</key><true/>
</dict></plist>
PLIST
launchctl load ~/Library/LaunchAgents/com.makakoo.mcp.http.plist
```

Ad-hoc foreground:

```bash
MAKAKOO_HOME=$HOME/MAKAKOO \
  PYTHONPATH=$HOME/makakoo-os/plugins-core/lib-harvey-core/src \
  python3 -m core.mcp.http_shim
```

## Wire protocol

```
POST /rpc
X-Makakoo-Peer:   <name>
X-Makakoo-Ts:     <unix-millis>                 (±60s drift window)
X-Makakoo-Sig:    ed25519=<base64(sig)>
X-Makakoo-Nonce:  <id>                          (required — echoed into brain writes)
Body: <JSON-RPC 2.0 request>

canonical_digest = SHA256(body_bytes || ts_decimal_ascii)
signature        = Ed25519.sign(canonical_digest)
```

Trust file at `$MAKAKOO_HOME/config/peers/trusted.keys`. One line per peer:

```
<peer-name> <base64-32-byte-pubkey>
```

## brain_write_journal nonce behavior

When a signed peer calls `tools/call` with `name=brain_write_journal`, the
shim appends ` {nonce=<id>}` to the end of the content line before writing
to the journal. `brain_tail` extracts that id on the way back out, so the
listener's nonce-aware LRU can drop its own writes without racing a timer.
Human-authored lines (Logseq, direct edits) have no nonce; they return
`nonce: null` and are always delivered to the listener.

## Configuration

| Env var | Default | Meaning |
|---|---|---|
| `MAKAKOO_MCP_HTTP_BIND` | `0.0.0.0` | Listen address. Use `127.0.0.1` for loopback-only. |
| `MAKAKOO_MCP_HTTP_PORT` | `8765`    | Port. |
| `MAKAKOO_MCP_POOL_SIZE` | `2`       | macOS stdio pool size. Benchmark before raising on Linux. |
| `MAKAKOO_MCP_BIN`       | `~/.cargo/bin/makakoo-mcp` | Path to the Rust MCP binary. |
| `MAKAKOO_HOME`          | `~/MAKAKOO` | Root used to resolve trust file, Brain, state dir. |

## Running the tests

```bash
# Unit / multiprocess flock test — no live shim needed
PYTHONPATH=plugins-core/lib-harvey-core/src \
  python3 plugins-core/lib-harvey-core/src/core/mcp/tests/test_http_shim_unit.py

# Listener LRU + nonce helpers (Node)
node plugins-core/lib-harvey-core/src/core/tests/test_harvey_listen.js

# Integration — requires a running shim on $MAKAKOO_MCP_HTTP_PORT
python3 plugins-core/lib-harvey-core/src/core/mcp/tests/test_http_shim_concurrency.py
```

## Agent usage

Start the peer stack:

```bash
makakoo agent start octopus-peer
```

The agent manages the shim's launchd/systemd unit and the listener
process together. Starting via the agent is the intended entry point
from `makakoo octopus bootstrap` (Phase 2) — direct invocation via
the snippets above is the fallback for debugging.

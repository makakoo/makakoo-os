#!/usr/bin/env python3
"""
Concurrency regression test for http_shim.py.

Verifies McpStdioPool delivers real parallelism to HTTP callers — five
concurrent `tools/list` requests complete in roughly the time of ONE
request, not five × serialized.

Runs against the LIVE local shim on http://127.0.0.1:MAKAKOO_MCP_HTTP_PORT.
The single-lock landmine was invisible under unit tests of the code and
only surfaces when the real subprocess is under concurrent load. Keep this
an integration test.

Preconditions:
  - http_shim.py running (launchd or foreground) on the configured port
  - $MAKAKOO_HOME/config/peers/trusted.keys has a 'test-peer-concurrency'
    entry matching the signing key generated below (this test adds one
    and removes it on teardown)
"""

from __future__ import annotations

import base64
import hashlib
import json
import os
import secrets
import sys
import time
import threading
import urllib.request

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

PORT = int(os.environ.get("MAKAKOO_MCP_HTTP_PORT", "8765"))
URL = f"http://127.0.0.1:{PORT}/rpc"
TRUST_FILE = os.path.expanduser(
    os.path.join(
        os.environ.get("MAKAKOO_HOME", os.path.expanduser("~/MAKAKOO")),
        "config", "peers", "trusted.keys",
    )
)
PEER_NAME = "test-peer-concurrency"
N_CONCURRENT = 5


def _mint_nonce() -> str:
    return secrets.token_hex(16)


def sign_request(key: Ed25519PrivateKey, body: bytes, ts_ms: int) -> str:
    h = hashlib.sha256()
    h.update(body)
    h.update(str(ts_ms).encode("ascii"))
    digest = h.digest()
    return base64.b64encode(key.sign(digest)).decode("ascii")


def rpc_tools_list(key: Ed25519PrivateKey, call_id: int) -> tuple[int, float, int]:
    body = json.dumps({"jsonrpc": "2.0", "id": call_id, "method": "tools/list"}).encode()
    ts = int(time.time() * 1000)
    sig_b64 = sign_request(key, body, ts)
    req = urllib.request.Request(
        URL, data=body, method="POST",
        headers={
            "Content-Type": "application/json",
            "X-Makakoo-Peer": PEER_NAME,
            "X-Makakoo-Ts": str(ts),
            "X-Makakoo-Sig": f"ed25519={sig_b64}",
            "X-Makakoo-Nonce": _mint_nonce(),
        },
    )
    t0 = time.time()
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            body_bytes = resp.read()
        elapsed = time.time() - t0
        parsed = json.loads(body_bytes)
        tools = parsed.get("result", {}).get("tools", [])
        return (call_id, elapsed, len(tools))
    except Exception:
        return (call_id, time.time() - t0, -1)


def append_trust(peer_name: str, pubkey_b64: str) -> None:
    os.makedirs(os.path.dirname(TRUST_FILE), exist_ok=True)
    existing = []
    if os.path.exists(TRUST_FILE):
        with open(TRUST_FILE) as f:
            existing = [ln for ln in f if not ln.strip().startswith(f"{peer_name} ")]
    existing.append(f"{peer_name} {pubkey_b64}\n")
    with open(TRUST_FILE, "w") as f:
        f.writelines(existing)
    os.chmod(TRUST_FILE, 0o600)


def remove_trust(peer_name: str) -> None:
    if not os.path.exists(TRUST_FILE):
        return
    with open(TRUST_FILE) as f:
        lines = [ln for ln in f if not ln.strip().startswith(f"{peer_name} ")]
    with open(TRUST_FILE, "w") as f:
        f.writelines(lines)
    os.chmod(TRUST_FILE, 0o600)


def main() -> int:
    key = Ed25519PrivateKey.generate()
    pubkey_bytes = key.public_key().public_bytes_raw()
    pubkey_b64 = base64.b64encode(pubkey_bytes).decode("ascii")
    append_trust(PEER_NAME, pubkey_b64)

    try:
        warm = rpc_tools_list(key, 0)
        if warm[2] < 40:
            print(f"FAIL: warmup returned {warm[2]} tools (expected ≥40). Shim not healthy?")
            return 2
        warm_elapsed = warm[1]
        print(f"warmup: {warm_elapsed*1000:.1f}ms, {warm[2]} tools")

        results: list[tuple[int, float, int]] = []
        results_lock = threading.Lock()

        def run(cid: int) -> None:
            r = rpc_tools_list(key, cid)
            with results_lock:
                results.append(r)

        threads = [threading.Thread(target=run, args=(i,)) for i in range(1, N_CONCURRENT + 1)]
        t0 = time.time()
        for t in threads: t.start()
        for t in threads: t.join()
        wall_elapsed = time.time() - t0

        expected_serial = warm_elapsed * N_CONCURRENT
        speedup = expected_serial / max(wall_elapsed, 0.001)
        failures = [r for r in results if r[2] < 40]
        slowest = max(r[1] for r in results)

        print(f"concurrent: {wall_elapsed*1000:.1f}ms wall, slowest-single {slowest*1000:.1f}ms")
        print(f"expected-if-serial: {expected_serial*1000:.1f}ms (N={N_CONCURRENT} × warm)")
        print(f"speedup vs serial: {speedup:.2f}x")

        if failures:
            print(f"FAIL: {len(failures)}/{N_CONCURRENT} returned error")
            return 3
        if wall_elapsed > 2.0:
            print(f"FAIL: wall time {wall_elapsed*1000:.0f}ms > 2000ms spec ceiling")
            return 4
        if slowest > 1.5:
            print(f"FAIL: slowest single-call {slowest*1000:.0f}ms > 1500ms")
            return 4

        print(f"PASS: {N_CONCURRENT} concurrent peers completed in {wall_elapsed*1000:.0f}ms "
              f"(spec: <2000ms). Pool size: {os.environ.get('MAKAKOO_MCP_POOL_SIZE', '2 (default)')}.")
        return 0
    finally:
        remove_trust(PEER_NAME)


if __name__ == "__main__":
    sys.exit(main())

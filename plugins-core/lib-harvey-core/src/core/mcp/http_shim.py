#!/usr/bin/env python3
"""
makakoo-mcp-http-shim — Python HTTP front-end for makakoo-mcp.

Why this exists
---------------
`makakoo-mcp --http 0.0.0.0:8765` works fine when reached via 127.0.0.1,
but connections arriving through a macOS utun interface (WireGuard tunnel
from a Tytus pod or SME teammate) never reach axum's request handler:
tokio/mio's kqueue-backed accept() silently drops the readiness event for
sockets landing on utun interfaces. Python's `socket.accept()` handles the
same traffic fine. Sidestepping tokio by running the HTTP front-end in
Python makes pod → Mac MCP calls work without patching tokio.

This is **load-bearing for Harvey Octopus**. Do not replace with axum.

Wire format
-----------
Identical to makakoo-mcp's --http mode, plus a mandatory nonce header
introduced in Octopus Phase 1 for LRU-based self-ack filtering:

    POST /rpc
    X-Makakoo-Peer:   <name>
    X-Makakoo-Ts:     <unix-millis>
    X-Makakoo-Sig:    ed25519=<base64sig>
    X-Makakoo-Nonce:  <id>   (required)
    body: <JSON-RPC 2.0 request>

    canonical_digest = SHA256(body || ts_decimal_ascii)
    signature = Ed25519.sign(canonical_digest)  # 64 bytes → base64

Nonce is echoed into the journal line on `brain_write_journal` so
autonomous listeners can drop their own writes via a nonce-aware LRU cache
instead of a brittle timer filter.

Trust file: `$MAKAKOO_HOME/config/peers/trusted.keys`
    Each line: `<peer-name> <base64-32-byte-pubkey>`
    The file is re-read on every request (mtime-keyed cache) so
    `launchctl kickstart -k` is NOT required after adding a peer.

Concurrency & durability
------------------------
- **Stdio pool:** opens N long-lived `makakoo-mcp` stdio subprocesses at
  startup (N from `MAKAKOO_MCP_POOL_SIZE`, default 2 — macOS benchmark
  showed N=3 is 6x slower per call due to pipe + tokio runtime contention).
  Each authenticated request checks out a worker from the pool.
- **Advisory file-locking:** any shim-handled tool that appends to a Brain
  file uses `fcntl.flock(LOCK_EX)` under `_brain_write_flock()`. Five SME
  peers writing concurrently cannot produce interleaved or corrupted JSON
  objects — each writer takes the lock, appends its bytes, releases.
  Critical for SME scaling (Phase 4 requires 10 peers × 30 writes/min).
"""

from __future__ import annotations

import base64
import contextlib
import datetime
import fcntl
import hashlib
import json
import logging
import os
import queue
import socketserver
import subprocess
import sys
import threading
import time
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
from cryptography.exceptions import InvalidSignature

# Phase 4 enforcement layer — scope + per-peer write rate limit. Soft-
# import so the shim stays usable even when `core.octopus` isn't on
# PYTHONPATH (e.g. legacy installs that only ship the mcp subtree).
try:
    from core.octopus.enforce import enforce_request  # type: ignore
    from core.octopus.ratelimit import PeerRateLimiter  # type: ignore
    _ENFORCE_AVAILABLE = True
except ImportError:
    _ENFORCE_AVAILABLE = False
    enforce_request = None  # type: ignore
    PeerRateLimiter = None  # type: ignore

# ────────────────────────── config ──────────────────────────────────

BIND_HOST = os.environ.get("MAKAKOO_MCP_HTTP_BIND", "0.0.0.0")
BIND_PORT = int(os.environ.get("MAKAKOO_MCP_HTTP_PORT", "8765"))
DRIFT_WINDOW_MS = 60_000  # matches makakoo_core::adapter::peer::DRIFT_WINDOW_MS
SIG_PREFIX = "ed25519="
MAKAKOO_HOME = os.environ.get("MAKAKOO_HOME", os.path.expanduser("~/MAKAKOO"))
TRUST_FILE = os.path.join(MAKAKOO_HOME, "config", "peers", "trusted.keys")
MAKAKOO_MCP_BIN = os.environ.get(
    "MAKAKOO_MCP_BIN", os.path.expanduser("~/.cargo/bin/makakoo-mcp")
)
POOL_SIZE = max(1, int(os.environ.get("MAKAKOO_MCP_POOL_SIZE", "2")))
POOL_ACQUIRE_TIMEOUT_S = float(os.environ.get("MAKAKOO_MCP_POOL_ACQUIRE_TIMEOUT_S", "30"))
# N=2 is the macOS sweet spot (see benchmark in test_http_shim_concurrency).
# Bump for Linux after re-benchmarking.

log = logging.getLogger("makakoo-mcp-shim")
log.setLevel(logging.DEBUG if os.environ.get("MAKAKOO_MCP_SHIM_DEBUG") else logging.INFO)
handler = logging.StreamHandler(sys.stderr)
handler.setFormatter(logging.Formatter("%(asctime)s %(levelname)s %(message)s"))
log.addHandler(handler)


# ────────────────────────── trust file ──────────────────────────────

_trust_cache: tuple[float, dict[str, Ed25519PublicKey]] | None = None
_trust_lock = threading.Lock()


def load_trust() -> dict[str, Ed25519PublicKey]:
    """Return parsed trust file using an mtime-keyed cache.

    Reparses only when the file changes on disk. Under concurrent load
    (10 peers hitting the shim at once) this avoids N× file I/O + base64
    decode + Ed25519 key construction per wave.
    """
    global _trust_cache
    try:
        mtime = os.path.getmtime(TRUST_FILE)
    except FileNotFoundError:
        mtime = 0.0
    cached = _trust_cache
    if cached is not None and cached[0] == mtime:
        return cached[1]

    parsed: dict[str, Ed25519PublicKey] = {}
    try:
        with open(TRUST_FILE, "r") as f:
            for line in f:
                line = line.strip()
                if not line or line.startswith("#"):
                    continue
                parts = line.split()
                if len(parts) < 2:
                    continue
                name, b64 = parts[0], parts[1]
                try:
                    raw = base64.b64decode(b64)
                    if len(raw) != 32:
                        continue
                    parsed[name] = Ed25519PublicKey.from_public_bytes(raw)
                except Exception:
                    continue
    except FileNotFoundError:
        pass
    with _trust_lock:
        _trust_cache = (mtime, parsed)
    return parsed


# ────────────────────────── stdio dispatcher ────────────────────────

class McpStdio:
    """Long-lived makakoo-mcp subprocess speaking newline-delimited JSON-RPC.

    Per-worker lock serializes write → readline on ONE subprocess. N of
    these run in parallel under `McpStdioPool`, so the system-level
    concurrency is N, not 1.
    """

    _next_id = 0
    _next_id_lock = threading.Lock()

    def __init__(self, bin_path: str):
        self.bin_path = bin_path
        with McpStdio._next_id_lock:
            self.worker_id = McpStdio._next_id
            McpStdio._next_id += 1
        self.lock = threading.Lock()
        self.proc: subprocess.Popen | None = None
        self._spawn()

    def _spawn(self) -> None:
        env = os.environ.copy()
        env.setdefault("MAKAKOO_HOME", MAKAKOO_HOME)
        # Quiet child stderr to WARN unless explicitly overridden. Chatty
        # workers × drain threads × Python's logging lock was creating
        # ~10× per-call overhead in the Phase 2 pool benchmark.
        env.setdefault("RUST_LOG", os.environ.get("MAKAKOO_MCP_WORKER_LOG", "warn"))
        log.info("launching makakoo-mcp stdio worker #%d: %s", self.worker_id, self.bin_path)
        self.proc = subprocess.Popen(
            [self.bin_path],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            bufsize=0,
            env=env,
        )
        threading.Thread(
            target=self._drain_stderr,
            name=f"mcp-stderr-{self.worker_id}",
            daemon=True,
        ).start()

    def _drain_stderr(self) -> None:
        proc = self.proc
        if proc is None or proc.stderr is None:
            return
        for line in iter(proc.stderr.readline, b""):
            log.info("[mcp#%d] %s", self.worker_id, line.decode("utf-8", "replace").rstrip())

    def call(self, body: bytes) -> bytes:
        with self.lock:
            if self.proc is None or self.proc.poll() is not None:
                old_exit = self.proc.returncode if self.proc else None
                log.warning("mcp worker #%d died (exit %s) — respawning", self.worker_id, old_exit)
                self._spawn()
            assert self.proc is not None and self.proc.stdin is not None and self.proc.stdout is not None
            self.proc.stdin.write(body.strip() + b"\n")
            self.proc.stdin.flush()
            line = self.proc.stdout.readline()
            if not line:
                try:
                    if self.proc:
                        self.proc.kill()
                except Exception:
                    pass
                raise RuntimeError(f"mcp worker #{self.worker_id} stdout closed mid-call")
            return line.rstrip(b"\n")


class McpStdioPool:
    """Fixed-size pool of `McpStdio` workers with thread-safe checkout.

    Bounds the blast radius of a slow tool (`harvey_knowledge_ingest` on a
    large PDF) to ONE worker while the rest serve fast traffic. Callers
    queue when all workers are busy, which is the correct back-pressure
    shape for an SME team hammering the brain.
    """

    def __init__(self, bin_path: str, size: int):
        if size < 1:
            raise ValueError(f"pool size must be ≥ 1 (got {size})")
        self.size = size
        self.bin_path = bin_path
        self._workers = [McpStdio(bin_path) for _ in range(size)]
        self._available: queue.Queue[McpStdio] = queue.Queue()
        for w in self._workers:
            self._available.put(w)
        log.info("McpStdioPool up with %d workers", size)

    def call(self, body: bytes, timeout: float | None = None) -> bytes:
        acq_t0 = time.time()
        worker = self._available.get(
            timeout=timeout if timeout is not None else POOL_ACQUIRE_TIMEOUT_S,
        )
        acq_elapsed = time.time() - acq_t0
        call_t0 = time.time()
        try:
            return worker.call(body)
        finally:
            call_elapsed = time.time() - call_t0
            log.debug(
                "worker #%d acq=%.1fms call=%.1fms",
                worker.worker_id, acq_elapsed * 1000, call_elapsed * 1000,
            )
            self._available.put(worker)


# Lazy pool: instantiated on first dispatch so that (a) `http_shim.py` is
# importable in tests that only exercise the Python-side intercepts
# (brain_write_journal under flock, brain_tail) without requiring the
# Rust binary to be installed, and (b) import cost stays flat whether
# the shim is run as `__main__` or imported by the test harness.

_mcp: McpStdioPool | None = None
_mcp_init_lock = threading.Lock()


def _get_mcp() -> McpStdioPool:
    global _mcp
    if _mcp is not None:
        return _mcp
    with _mcp_init_lock:
        if _mcp is None:
            _mcp = McpStdioPool(MAKAKOO_MCP_BIN, POOL_SIZE)
    return _mcp


# Process-wide enforcement state (Phase 4). Created on first request so
# the shim stays import-light for unit tests that don't exercise
# dispatch.
_limiter: "PeerRateLimiter | None" = None


def _get_limiter():
    """Return the shared :class:`PeerRateLimiter`, or None when
    enforcement is unavailable (soft-import failed). Callers treat None
    as "no rate limit" — Phase 1 behavior."""
    global _limiter
    if not _ENFORCE_AVAILABLE:
        return None
    if _limiter is None:
        _limiter = PeerRateLimiter()  # type: ignore[misc]
    return _limiter


# ────────────────────────── brain write interlock ───────────────────

# Shim-handled MCP tools that append to Brain files (currently just the
# `brain_write_journal` intercept below). All such appends acquire an
# advisory flock on a sentinel file under `$MAKAKOO_HOME/state/octopus/`.
# The lock is at the shim level — not per-peer — because SME mode has
# N peers hitting the SAME `~/MAKAKOO/data/Brain/journals/YYYY_MM_DD.md`
# concurrently. POSIX append is atomic for small writes on local FS, but
# a) nobody guarantees your brain journal sits on a local FS forever
# (iCloud Drive, network mount), and b) we also want an interlock that a
# future cross-host write path can participate in. The sentinel file is
# cheap, predictable, portable.

_BRAIN_FLOCK_DIR = os.path.join(MAKAKOO_HOME, "state", "octopus")
_BRAIN_FLOCK_PATH = os.path.join(_BRAIN_FLOCK_DIR, "brain-write.lock")


@contextlib.contextmanager
def _brain_write_flock():
    """Acquire an exclusive advisory flock for the duration of a Brain write.

    Opens (and lazily creates) the sentinel, blocks on `LOCK_EX`, yields,
    then releases. The sentinel file itself is a zero-byte fd — writers
    never touch its contents.
    """
    os.makedirs(_BRAIN_FLOCK_DIR, exist_ok=True)
    fd = os.open(_BRAIN_FLOCK_PATH, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(fd, fcntl.LOCK_UN)
    finally:
        os.close(fd)


# ────────────────────────── brain_tail + write intercepts ────────────

# `brain_tail` and (new) `brain_write_journal` are handled in Python for
# two reasons: (a) they ship today rather than waiting on a Rust tool
# reshuffle, (b) they need to participate in the flock interlock and the
# nonce-injection path, which are both shim-level concerns. The
# architecturally-right destination remains `makakoo-core`; migrate once
# another subsystem needs the same primitives.

# Import lazily at call time to avoid loading the parent package eagerly
# (shim is often invoked directly as a script without the package path
# configured; a top-of-file `from core.brain_tail import ...` would fail).
def _load_brain_tail():
    from core.brain_tail import brain_tail, extract_nonce, BRAIN_JOURNALS_DIR  # type: ignore
    return brain_tail, extract_nonce, BRAIN_JOURNALS_DIR


def _brain_journal_path_today() -> str:
    _, _, journals_dir = _load_brain_tail()
    today = datetime.date.today().strftime("%Y_%m_%d")
    return os.path.join(journals_dir, f"{today}.md")


def _nonce_suffix(nonce: str | None) -> str:
    """Render the trailing `{nonce=<id>}` token if a nonce is present.

    Returns empty string when nonce is None. Strips any whitespace and
    cosmetic braces so `brain_tail.extract_nonce` finds it reliably on
    the other side.
    """
    if not nonce:
        return ""
    safe = "".join(ch for ch in nonce if ch.isalnum() or ch in "-_")
    if not safe:
        return ""
    return f" {{nonce={safe}}}"


def _write_journal_line(content: str, nonce: str | None) -> None:
    """Append a line to today's journal under the Brain flock.

    `content` may contain its own leading `- ` marker (Logseq outliner
    convention) or not; we do not rewrite the prefix. The nonce suffix
    is appended after the content, before the trailing newline, so
    tail-based extractors find it on exactly the line that was written.
    """
    path = _brain_journal_path_today()
    os.makedirs(os.path.dirname(path), exist_ok=True)
    suffix = _nonce_suffix(nonce)
    line = content.rstrip("\n") + suffix + "\n"
    with _brain_write_flock():
        with open(path, "a", encoding="utf-8") as f:
            f.write(line)


def _handle_brain_write_journal(rpc: dict, nonce: str | None) -> dict:
    """Append the provided content to today's journal and return a JSON-RPC
    response. Handled in Python so nonce injection happens on the write
    side of the pipe (the Rust tool would need to know about the header,
    which is a shim-level concept)."""
    params = rpc.get("params") or {}
    args = params.get("arguments") or {}
    content = args.get("content") or args.get("line")
    if not isinstance(content, str) or not content:
        return {
            "jsonrpc": "2.0",
            "id": rpc.get("id"),
            "error": {"code": -32602, "message": "'content' is required (non-empty string)"},
        }
    try:
        _write_journal_line(content, nonce)
    except Exception as exc:
        log.error("brain_write_journal failed: %s", exc)
        return {
            "jsonrpc": "2.0",
            "id": rpc.get("id"),
            "error": {"code": -32000, "message": f"brain_write_journal: {exc}"},
        }
    return {
        "jsonrpc": "2.0",
        "id": rpc.get("id"),
        "result": {"content": [{"type": "text", "text": "journal updated"}]},
    }


def _handle_brain_tail(rpc: dict) -> dict:
    brain_tail, _, _ = _load_brain_tail()
    params = rpc.get("params") or {}
    args = params.get("arguments") or {}
    pattern = args.get("pattern", "")
    cursor_date = args.get("cursor_date")
    cursor_line = int(args.get("cursor_line") or 0)
    include_yesterday = bool(args.get("include_yesterday", True))

    if not isinstance(pattern, str) or not pattern:
        return {
            "jsonrpc": "2.0",
            "id": rpc.get("id"),
            "error": {"code": -32602, "message": "'pattern' is required (non-empty string)"},
        }
    if cursor_date and not isinstance(cursor_date, str):
        return {
            "jsonrpc": "2.0",
            "id": rpc.get("id"),
            "error": {"code": -32602, "message": "'cursor_date' must be 'YYYY_MM_DD'"},
        }

    try:
        body = brain_tail(pattern, cursor_date, cursor_line, include_yesterday)
        return {
            "jsonrpc": "2.0",
            "id": rpc.get("id"),
            "result": {"content": [{"type": "text", "text": json.dumps(body)}]},
        }
    except Exception as exc:
        log.error("brain_tail failed: %s", exc)
        return {
            "jsonrpc": "2.0",
            "id": rpc.get("id"),
            "error": {"code": -32000, "message": f"brain_tail: {exc}"},
        }


def _handle_tools_call_intercept(rpc: dict, nonce: str | None) -> dict | None:
    """Intercept `tools/call` names handled directly in Python.

    Returns a JSON-RPC response dict to send back, or None to forward to
    the makakoo-mcp stdio pool as usual.
    """
    if rpc.get("method") != "tools/call":
        return None
    params = rpc.get("params") or {}
    name = params.get("name")
    if name == "brain_tail":
        return _handle_brain_tail(rpc)
    if name == "brain_write_journal":
        return _handle_brain_write_journal(rpc, nonce)
    return None


# ────────────────────────── HTTP handler ────────────────────────────

class RpcHandler(BaseHTTPRequestHandler):
    server_version = "makakoo-mcp-shim/0.2"

    def log_message(self, fmt: str, *args) -> None:  # suppress default access log
        return

    def _send_json(self, status: int, obj: dict) -> None:
        body = json.dumps(obj).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)

    def _bad_request(self, msg: str) -> None:
        self._send_json(400, {"error": msg})

    def _unauthorized(self, msg: str) -> None:
        self._send_json(401, {"error": msg})

    def do_POST(self) -> None:
        if self.path != "/rpc":
            self._send_json(404, {"error": "not found"})
            return

        peer = self.headers.get("X-Makakoo-Peer")
        ts_str = self.headers.get("X-Makakoo-Ts")
        sig_header = self.headers.get("X-Makakoo-Sig")
        nonce = self.headers.get("X-Makakoo-Nonce")

        if not peer:
            self._bad_request("X-Makakoo-Peer header required"); return
        if not ts_str:
            self._bad_request("X-Makakoo-Ts header required"); return
        if not sig_header:
            self._bad_request("X-Makakoo-Sig header required"); return
        if not nonce:
            self._bad_request("X-Makakoo-Nonce header required"); return
        try:
            ts = int(ts_str)
        except ValueError:
            self._bad_request("X-Makakoo-Ts must be a unix-millis integer"); return

        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length) if length > 0 else b""

        now = int(time.time() * 1000)
        drift = abs(now - ts)
        if drift > DRIFT_WINDOW_MS:
            self._unauthorized(f"clock drift {drift}ms exceeds window {DRIFT_WINDOW_MS}ms")
            return

        trust = load_trust()
        pub = trust.get(peer)
        if pub is None:
            self._unauthorized(f"unknown peer: {peer}"); return

        if not sig_header.startswith(SIG_PREFIX):
            self._unauthorized(f"signature must start with {SIG_PREFIX!r}"); return
        try:
            sig_bytes = base64.b64decode(sig_header[len(SIG_PREFIX):])
        except Exception:
            self._unauthorized("signature base64 invalid"); return
        if len(sig_bytes) != 64:
            self._unauthorized(f"signature must be 64 bytes, got {len(sig_bytes)}"); return

        h = hashlib.sha256()
        h.update(body)
        h.update(str(ts).encode("ascii"))
        digest = h.digest()
        try:
            pub.verify(sig_bytes, digest)
        except InvalidSignature:
            self._unauthorized("signature verification failed"); return

        try:
            rpc_obj = json.loads(body)
        except Exception:
            rpc_obj = None

        # Phase 4: scope + per-peer write rate limit. Runs AFTER
        # signature verification (the peer is authenticated) and BEFORE
        # dispatch (so a denied write doesn't touch the stdio pool or
        # the brain write path).
        limiter = _get_limiter()
        if rpc_obj is not None and limiter is not None and enforce_request is not None:
            try:
                decision = enforce_request(
                    peer_name=peer,
                    rpc_method=rpc_obj.get("method", ""),
                    rpc_params=rpc_obj.get("params") or {},
                    limiter=limiter,
                )
            except Exception as exc:
                log.error("enforce_request raised: %s", exc)
                decision = None
            if decision is not None and not decision.allowed:
                status = decision.http_status
                body_out = {"error": decision.error_message}
                encoded = json.dumps(body_out).encode("utf-8")
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(encoded)))
                self.send_header("Connection", "close")
                if status == 429 and decision.retry_after_s > 0:
                    # Round up to whole seconds — RFC 7231 allows a
                    # fractional value but some clients reject it.
                    self.send_header("Retry-After", str(int(decision.retry_after_s) + 1))
                self.end_headers()
                self.wfile.write(encoded)
                return

        shim_response = (
            _handle_tools_call_intercept(rpc_obj, nonce) if rpc_obj else None
        )
        if shim_response is not None:
            resp = json.dumps(shim_response).encode("utf-8")
        else:
            try:
                resp = _get_mcp().call(body)
            except Exception as exc:
                log.error("mcp stdio call failed: %s", exc)
                self._send_json(500, {"error": f"mcp dispatch: {exc}"}); return

        if not resp.strip():
            self.send_response(204)
            self.send_header("Connection", "close")
            self.end_headers()
            return

        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(resp)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(resp)


class ThreadedTCPServer(socketserver.ThreadingMixIn, socketserver.TCPServer):
    allow_reuse_address = True
    daemon_threads = True


def main() -> None:
    # Warm the stdio pool eagerly when run as a server — we want any
    # binary-missing / config error to surface on startup, not on the
    # first request.
    _get_mcp()
    server = ThreadedTCPServer((BIND_HOST, BIND_PORT), RpcHandler)
    log.info(
        "makakoo-mcp-shim listening on %s:%d → %s  (trust file: %s, pool size: %d)",
        BIND_HOST, BIND_PORT, MAKAKOO_MCP_BIN, TRUST_FILE, POOL_SIZE,
    )
    if BIND_HOST not in ("127.0.0.1", "::1"):
        log.warning(
            "shim bound to non-loopback %s — Ed25519 peer auth is mandatory, "
            "but make sure your network posture matches your intent.",
            BIND_HOST,
        )
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        log.info("shutting down")
        server.shutdown()


if __name__ == "__main__":
    main()

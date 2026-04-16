"""Python client for the Makakoo kernel capability socket.

Mirrors the Rust ``makakoo-client`` crate: typed methods for state +
secrets, ``CapabilityDenied`` as a distinct exception so plugins can
tell "I shouldn't have tried that" from real server errors.

Wire protocol: newline-delimited JSON-RPC over a Unix domain socket.
Bytes are base64-encoded in the JSON envelope for state operations.

Pure stdlib — uses ``socket``, ``json``, ``base64``, ``os``, ``threading``.
"""

from __future__ import annotations

import base64
import json
import os
import socket
import threading
from dataclasses import dataclass
from typing import Any, List, Optional

__all__ = [
    "CapabilityDenied",
    "ClientError",
    "ServerError",
    "StateEntry",
    "Client",
]


class ClientError(Exception):
    """Base class for every error raised by :class:`Client`."""


class CapabilityDenied(ClientError):
    """Raised when the kernel refuses a call because the plugin's
    ``[capabilities].grants`` do not cover the requested verb + scope.
    The JSON-RPC code for this is ``-32001``.
    """

    def __init__(self, verb: str, scope: str, reason: str) -> None:
        super().__init__(f"capability denied {verb}:{scope}: {reason}")
        self.verb = verb
        self.scope = scope
        self.reason = reason


class ServerError(ClientError):
    """Raised for any non-denial error response from the server."""

    def __init__(self, code: int, message: str) -> None:
        super().__init__(f"server error (code {code}): {message}")
        self.code = code
        self.message = message


@dataclass
class StateEntry:
    name: str
    is_dir: bool


class Client:
    """Connected client handle.

    Construct via :meth:`connect` or :meth:`connect_from_env`. Every
    call serialises through an internal lock so concurrent callers
    sharing one client don't interleave writes on the socket.
    """

    def __init__(self, sock: socket.socket, socket_path: str) -> None:
        self._sock = sock
        self._socket_path = socket_path
        self._reader = sock.makefile("rb")
        self._lock = threading.Lock()
        self._next_id = 1
        self._correlation_id: Optional[str] = None

    # ── construction ─────────────────────────────────────────────────

    @classmethod
    def connect(cls, path: str) -> "Client":
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.connect(path)
        return cls(s, path)

    @classmethod
    def connect_from_env(cls) -> "Client":
        path = os.environ.get("MAKAKOO_SOCKET_PATH")
        if not path:
            raise ClientError(
                "capability socket path not set (checked $MAKAKOO_SOCKET_PATH)"
            )
        return cls.connect(path)

    # ── housekeeping ─────────────────────────────────────────────────

    @property
    def socket_path(self) -> str:
        return self._socket_path

    def with_correlation_id(self, cid: str) -> "Client":
        """Attach a correlation id to every subsequent request. Useful
        for grouping a multi-step plugin action in the audit log.
        """
        self._correlation_id = cid
        return self

    def close(self) -> None:
        try:
            self._reader.close()
        finally:
            self._sock.close()

    def __enter__(self) -> "Client":
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.close()

    # ── wire ─────────────────────────────────────────────────────────

    def _call(
        self,
        method: str,
        verb: str,
        scope: str,
        params: dict,
    ) -> Any:
        with self._lock:
            req_id = self._next_id
            self._next_id += 1
            req = {
                "id": req_id,
                "method": method,
                "params": params,
                "verb": verb,
            }
            if scope:
                req["scope"] = scope
            if self._correlation_id is not None:
                req["correlation_id"] = self._correlation_id
            line = (json.dumps(req) + "\n").encode("utf-8")
            self._sock.sendall(line)

            raw = self._reader.readline()
            if not raw:
                raise ClientError("server closed connection before replying")
            resp = json.loads(raw.decode("utf-8"))

        if "error" in resp and resp["error"] is not None:
            err = resp["error"]
            code = int(err.get("code", 0))
            msg = str(err.get("message", ""))
            if code == -32001:
                reason = ""
                data = err.get("data")
                if isinstance(data, dict):
                    reason = str(data.get("reason", ""))
                raise CapabilityDenied(verb, scope, reason)
            raise ServerError(code, msg)
        if "result" not in resp or resp["result"] is None:
            raise ClientError("server returned neither result nor error")
        return resp["result"]

    # ── state ────────────────────────────────────────────────────────

    def state_read(self, path: str) -> bytes:
        """Read bytes from ``path`` under the plugin's state dir."""
        r = self._call(
            "state.read",
            "state/plugin",
            "",
            {"path": path},
        )
        b64 = r.get("bytes_b64")
        if not isinstance(b64, str):
            raise ClientError("malformed state.read response")
        return base64.b64decode(b64)

    def state_write(self, path: str, data: bytes) -> int:
        """Write ``data`` to ``path`` under the plugin's state dir."""
        encoded = base64.b64encode(data).decode("ascii")
        r = self._call(
            "state.write",
            "state/plugin",
            "",
            {"path": path, "bytes_b64": encoded},
        )
        return int(r.get("bytes_written", 0))

    def state_list(self, path: Optional[str] = None) -> List[StateEntry]:
        params: dict = {}
        if path:
            params["path"] = path
        r = self._call("state.list", "state/plugin", "", params)
        entries_raw = r.get("entries", [])
        if not isinstance(entries_raw, list):
            raise ClientError("malformed state.list response")
        out: List[StateEntry] = []
        for e in entries_raw:
            out.append(
                StateEntry(
                    name=str(e.get("name", "")),
                    is_dir=bool(e.get("is_dir", False)),
                )
            )
        return out

    def state_delete(self, path: str) -> bool:
        r = self._call(
            "state.delete",
            "state/plugin",
            "",
            {"path": path},
        )
        return bool(r.get("removed", False))

    # ── secrets ──────────────────────────────────────────────────────

    def secret_read(self, name: str) -> str:
        """Read a secret value by key name. The plugin must declare
        ``secrets/read:<NAME>`` in its manifest; otherwise the kernel
        raises :class:`CapabilityDenied`.
        """
        r = self._call(
            "secrets.read",
            "secrets/read",
            name,
            {"name": name},
        )
        value = r.get("value")
        if not isinstance(value, str):
            raise ClientError("malformed secrets.read response")
        return value

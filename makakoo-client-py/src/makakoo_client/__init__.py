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

    # ── brain ────────────────────────────────────────────────────────

    def brain_search(self, query: str, limit: int = 10) -> list[dict]:
        """FTS over Brain content. Returns a list of hits."""
        r = self._call(
            "brain.search",
            "brain/read",
            "",
            {"query": query, "limit": limit},
        )
        hits = r.get("hits", [])
        if not isinstance(hits, list):
            raise ClientError("malformed brain.search response")
        return hits

    def brain_recent(
        self, limit: int = 10, doc_type: Optional[str] = None
    ) -> list[dict]:
        """Most recent Brain documents, optionally filtered by type."""
        params: dict = {"limit": limit}
        if doc_type is not None:
            params["doc_type"] = doc_type
        r = self._call("brain.recent", "brain/read", "", params)
        hits = r.get("hits", [])
        if not isinstance(hits, list):
            raise ClientError("malformed brain.recent response")
        return hits

    def brain_read(self, doc_id: str) -> Optional[dict]:
        """Fetch one Brain document by id. Returns ``None`` if missing."""
        r = self._call(
            "brain.read",
            "brain/read",
            "",
            {"doc_id": doc_id},
        )
        doc = r.get("doc")
        return doc if isinstance(doc, dict) else None

    def brain_write_journal(self, line: str) -> str:
        """Append one line to today's Brain journal. Returns the path
        the line was written to. The kernel auto-prefixes ``- `` if the
        line doesn't already start with it.
        """
        r = self._call(
            "brain.write_journal",
            "brain/write",
            "",
            {"line": line},
        )
        path = r.get("appended_to")
        if not isinstance(path, str):
            raise ClientError("malformed brain.write_journal response")
        return path

    # ── llm ──────────────────────────────────────────────────────────

    def llm_chat(
        self,
        model: str,
        messages: list[tuple[str, str]] | list[dict],
    ) -> str:
        """Chat completion. ``messages`` accepts either ``[(role,
        content), …]`` or ``[{"role": ..., "content": ...}, …]``. Plugin
        must declare ``llm/chat:<model-glob>`` for the requested model.
        """
        msgs_norm: list[dict] = []
        for m in messages:
            if isinstance(m, tuple) and len(m) == 2:
                msgs_norm.append({"role": m[0], "content": m[1]})
            elif isinstance(m, dict):
                msgs_norm.append(m)
            else:
                raise ClientError(f"bad chat message shape: {m!r}")
        r = self._call(
            "llm.chat",
            "llm/chat",
            model,
            {"model": model, "messages": msgs_norm},
        )
        content = r.get("content")
        if not isinstance(content, str):
            raise ClientError("malformed llm.chat response")
        return content

    def llm_embed(self, text: str) -> list[float]:
        """Generate an embedding vector for ``text``. Plugin must
        declare ``llm/embed`` in its manifest.
        """
        r = self._call(
            "llm.embed",
            "llm/embed",
            "",
            {"text": text},
        )
        vec = r.get("embedding")
        if not isinstance(vec, list):
            raise ClientError("malformed llm.embed response")
        return [float(v) for v in vec]

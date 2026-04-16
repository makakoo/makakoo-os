"""Python client protocol-level tests.

Exercises the JSON-RPC framing + exception mapping against a mock
Unix socket. The real cross-language round-trip test lives on the
Rust side at ``makakoo-client/tests/py_e2e_socket.rs`` — this file
focuses on the parts that are hard to cover from there (malformed
server responses, deny variant routing).

Run: ``python3 -m pytest tests/`` from ``makakoo-client-py/``.
"""

from __future__ import annotations

import base64
import json
import os
import socket
import sys
import tempfile
import threading
from pathlib import Path
from typing import Callable, List, Optional

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from makakoo_client import (  # noqa: E402
    CapabilityDenied,
    Client,
    ClientError,
    ServerError,
)


def _make_fake_server(
    responder: Callable[[dict], Optional[dict]],
) -> tuple[str, threading.Thread, List[dict]]:
    """Start a throwaway Unix-socket server that calls ``responder`` for
    each incoming request. Returns the socket path + the server thread
    + a shared list of requests seen.
    """
    tmp = tempfile.mkdtemp()
    path = os.path.join(tmp, "test.sock")
    seen: List[dict] = []
    ready = threading.Event()

    def run() -> None:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.bind(path)
        s.listen(1)
        ready.set()
        conn, _ = s.accept()
        reader = conn.makefile("rb")
        writer = conn.makefile("wb")
        try:
            while True:
                line = reader.readline()
                if not line:
                    break
                req = json.loads(line)
                seen.append(req)
                resp = responder(req)
                if resp is None:
                    break
                writer.write((json.dumps(resp) + "\n").encode())
                writer.flush()
        finally:
            reader.close()
            writer.close()
            conn.close()
            s.close()
            try:
                os.remove(path)
            except OSError:
                pass

    t = threading.Thread(target=run, daemon=True)
    t.start()
    ready.wait(timeout=2.0)
    return path, t, seen


def test_state_write_then_read_roundtrip() -> None:
    data = b"hello"
    encoded = base64.b64encode(data).decode("ascii")
    stored: dict = {}

    def responder(req: dict) -> dict:
        m = req["method"]
        if m == "state.write":
            stored[req["params"]["path"]] = req["params"]["bytes_b64"]
            return {"id": req["id"], "result": {"bytes_written": len(data)}}
        if m == "state.read":
            b64 = stored[req["params"]["path"]]
            return {
                "id": req["id"],
                "result": {"bytes_b64": b64, "len": len(data)},
            }
        return {"id": req["id"], "error": {"code": -32000, "message": "nope"}}

    path, _, seen = _make_fake_server(responder)
    c = Client.connect(path)
    assert c.state_write("x.txt", data) == len(data)
    assert c.state_read("x.txt") == data
    c.close()
    assert seen[0]["method"] == "state.write"
    assert seen[0]["verb"] == "state/plugin"
    assert seen[1]["method"] == "state.read"
    # Params round-tripped the b64 correctly.
    assert seen[0]["params"]["bytes_b64"] == encoded


def test_denied_response_raises_capability_denied() -> None:
    def responder(req: dict) -> dict:
        return {
            "id": req["id"],
            "error": {
                "code": -32001,
                "message": "capability denied: net/http:https://evil",
                "data": {"reason": "no granted scope matches"},
            },
        }

    path, _, _ = _make_fake_server(responder)
    c = Client.connect(path)
    try:
        c.secret_read("MY_KEY")
    except CapabilityDenied as e:
        assert e.verb == "secrets/read"
        assert e.scope == "MY_KEY"
        assert "no granted scope" in e.reason
    else:
        raise AssertionError("expected CapabilityDenied")
    c.close()


def test_generic_server_error_raises_server_error() -> None:
    def responder(req: dict) -> dict:
        return {
            "id": req["id"],
            "error": {"code": -32000, "message": "backend blew up"},
        }

    path, _, _ = _make_fake_server(responder)
    c = Client.connect(path)
    try:
        c.state_read("boom.txt")
    except ServerError as e:
        assert e.code == -32000
        assert "backend blew up" in e.message
    else:
        raise AssertionError("expected ServerError")
    c.close()


def test_connect_from_env_missing_raises() -> None:
    os.environ.pop("MAKAKOO_SOCKET_PATH", None)
    try:
        Client.connect_from_env()
    except ClientError as e:
        assert "MAKAKOO_SOCKET_PATH" in str(e)
    else:
        raise AssertionError("expected ClientError")


def test_state_list_parses_entries() -> None:
    def responder(req: dict) -> dict:
        return {
            "id": req["id"],
            "result": {
                "entries": [
                    {"name": "alpha.txt", "is_dir": False},
                    {"name": "sub", "is_dir": True},
                ]
            },
        }

    path, _, _ = _make_fake_server(responder)
    c = Client.connect(path)
    out = c.state_list()
    assert [(e.name, e.is_dir) for e in out] == [
        ("alpha.txt", False),
        ("sub", True),
    ]
    c.close()


def test_correlation_id_is_sent_on_every_request() -> None:
    def responder(req: dict) -> dict:
        return {"id": req["id"], "result": {"bytes_written": 0}}

    path, _, seen = _make_fake_server(responder)
    c = Client.connect(path).with_correlation_id("trace-42")
    c.state_write("a.txt", b"")
    c.close()
    assert seen[0]["correlation_id"] == "trace-42"


if __name__ == "__main__":
    # Plain-Python runner so `python3 tests/test_protocol.py` works
    # without pytest or unittest boilerplate. Each `test_*` function in
    # module scope is called; the first AssertionError aborts with a
    # non-zero exit code so CI can tell us to fix it.
    import traceback

    funcs = [
        v
        for k, v in list(globals().items())
        if callable(v) and k.startswith("test_")
    ]
    passed = 0
    failed: list[tuple[str, str]] = []
    for fn in funcs:
        try:
            fn()
            print(f"  ok  {fn.__name__}")
            passed += 1
        except Exception:
            failed.append((fn.__name__, traceback.format_exc()))
            print(f"  FAIL {fn.__name__}")

    print(f"\npython unit tests: {passed} passed, {len(failed)} failed")
    if failed:
        for name, tb in failed:
            print(f"\n--- {name} ---\n{tb}")
        sys.exit(1)

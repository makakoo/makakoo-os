"""Tests for the ingest.py SSRF guard (`_check_url_allowed`).

`ingest.py` imports heavy optional deps (numpy, google.genai,
qdrant_client) at module load. We import them for real when available
and stub them otherwise, so the guard stays testable without the full
runtime installed.
"""

from __future__ import annotations

import socket
import sys
import types
from pathlib import Path

import pytest

_SRC = Path(__file__).resolve().parent.parent / "src"


def _ensure_importable(name: str) -> None:
    """Import `name`, or install empty stub modules if unavailable."""
    try:
        __import__(name)
        return
    except ImportError:
        pass
    parts = name.split(".")
    for i in range(1, len(parts) + 1):
        mod_name = ".".join(parts[:i])
        if mod_name in sys.modules:
            continue
        module = types.ModuleType(mod_name)
        sys.modules[mod_name] = module
        if i > 1:
            setattr(sys.modules[".".join(parts[: i - 1])], parts[i - 1], module)


def _load_ingest():
    _ensure_importable("numpy")
    _ensure_importable("google.genai.types")
    _ensure_importable("qdrant_client.http")

    import google.genai as genai

    if not hasattr(genai, "types"):
        genai.types = sys.modules["google.genai.types"]

    import qdrant_client

    if not hasattr(qdrant_client, "QdrantClient"):
        qdrant_client.QdrantClient = object
    if not hasattr(qdrant_client.http, "models"):
        qdrant_client.http.models = types.ModuleType("qdrant_client.http.models")

    if str(_SRC) not in sys.path:
        sys.path.insert(0, str(_SRC))
    import ingest

    return ingest


ingest = _load_ingest()


def _fake_getaddrinfo(*ips):
    def resolve(_host, port, *_args, **_kwargs):
        return [
            (socket.AF_INET, socket.SOCK_STREAM, 6, "", (ip, port)) for ip in ips
        ]

    return resolve


@pytest.mark.parametrize(
    "ip",
    ["127.0.0.1", "10.0.0.5", "192.168.1.1", "172.16.0.1", "169.254.1.1", "::1", "fc00::1"],
)
def test_rejects_non_public_resolved_addresses(monkeypatch, ip):
    monkeypatch.setattr(socket, "getaddrinfo", _fake_getaddrinfo(ip))
    with pytest.raises(RuntimeError, match="non-public"):
        ingest._check_url_allowed("http://example.com/file.pdf")


def test_allows_public_address_on_standard_ports(monkeypatch):
    monkeypatch.setattr(socket, "getaddrinfo", _fake_getaddrinfo("93.184.216.34"))
    ingest._check_url_allowed("http://example.com/file.pdf")
    ingest._check_url_allowed("https://example.com/file.pdf")
    ingest._check_url_allowed("http://example.com:80/file.pdf")
    ingest._check_url_allowed("https://example.com:443/file.pdf")


def test_rejects_nonstandard_port():
    # Rejected before any DNS resolution happens.
    with pytest.raises(RuntimeError, match="port"):
        ingest._check_url_allowed("http://example.com:8080/file.pdf")


def test_rejects_non_http_scheme():
    with pytest.raises(RuntimeError, match="scheme"):
        ingest._check_url_allowed("ftp://example.com/file.pdf")


def test_rejects_unresolvable_host(monkeypatch):
    def boom(_host, _port, *_args, **_kwargs):
        raise socket.gaierror("name or service not known")

    monkeypatch.setattr(socket, "getaddrinfo", boom)
    with pytest.raises(RuntimeError, match="cannot resolve"):
        ingest._check_url_allowed("http://nonexistent.invalid/file.pdf")


def test_download_http_invokes_guard(monkeypatch, tmp_path):
    monkeypatch.setattr(socket, "getaddrinfo", _fake_getaddrinfo("127.0.0.1"))
    with pytest.raises(RuntimeError, match="non-public"):
        ingest._download_http("http://169.254.169.254/latest/meta-data", tmp_path, lambda *_a: None)

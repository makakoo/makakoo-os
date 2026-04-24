"""mDNS discovery for Harvey Octopus peers.

Service type: ``_makakoo-peer._tcp.local.``. Every advertising peer
publishes:

    instance_name: <peer_name>._makakoo-peer._tcp.local.
    port:          <shim HTTP port (default 8765)>
    TXT:
        peer_name     = <peer_name>                          (redundant w/ instance)
        peer_pubkey   = <base64 32 bytes Ed25519>            (so joiners can pre-fill)
        v             = 1                                    (protocol version)
        scope_default = read-brain|write-brain|full-brain    (hint for invite UI)

The advertised record is enough to populate an invite-link's
``iss_pk`` / ``iss`` fields without a second round-trip. Joiners still
need the onboarding token (shared secret) to complete the handshake —
advertising peer names publicly is safe; advertising secrets is not.

Why a soft-import on zeroconf:
    The zeroconf package is ~1MB + a C-accelerated backend, and pods
    don't need it (they're onboarded via the invite URL + Tytus CIDR
    scan, not multicast). Rather than force zeroconf on every install,
    :func:`advertise` and :func:`discover` raise a clear ImportError
    with the pip-install hint when the module isn't present.

Tytus CIDR scan:
    Multicast is blocked over the WG tunnel, so mDNS doesn't reach
    pods. :func:`tytus_cidr_scan` sweeps the pod subnet
    (``10.42.42.0/24``) by best-effort ``/rpc``-probing each address
    and returning the responders. Cheap at 254 addresses × 200ms
    timeout × a few parallel workers.
"""

from __future__ import annotations

import concurrent.futures
import ipaddress
import logging
import socket
import time
from dataclasses import dataclass
from typing import Callable, Iterable

from . import DEFAULT_PEER_PORT, MAKAKOO_SERVICE_TYPE, TYTUS_POD_SUBNET

log = logging.getLogger("makakoo.octopus.discovery.mdns")

# ────────────────────────── zeroconf soft-import ────────────────────

try:
    from zeroconf import ServiceInfo, Zeroconf, ServiceBrowser, ServiceListener  # type: ignore
    _ZEROCONF_AVAILABLE = True
    _ZEROCONF_IMPORT_ERROR = None
except ImportError as _exc:  # pragma: no cover - depends on install
    _ZEROCONF_AVAILABLE = False
    _ZEROCONF_IMPORT_ERROR = _exc


def _require_zeroconf() -> None:
    if not _ZEROCONF_AVAILABLE:
        raise ImportError(
            "mDNS discovery requires the `zeroconf` package — install with "
            "`pip install 'zeroconf>=0.132'` on this host (or use invite-link "
            "onboarding instead, which does not require multicast). "
            f"Original import error: {_ZEROCONF_IMPORT_ERROR}"
        )


# ────────────────────────── service record ──────────────────────────

@dataclass(frozen=True)
class DiscoveredPeer:
    """One result from :func:`discover`.

    Attrs:
        peer_name: the peer's self-chosen mesh name.
        host: IP address (as string) advertised by the peer.
        port: TCP port the HTTP shim binds to.
        public_key_b64: peer's Ed25519 public key (base64, 32 raw bytes).
        scope_default: hint for the invite UI — what scope the peer
                       expects to be granted. The deciding peer is
                       still free to override.
        raw_txt: the parsed TXT dict, for callers that want to inspect
                 fields we don't wrap in typed accessors.
    """
    peer_name: str
    host: str
    port: int
    public_key_b64: str | None
    scope_default: str | None
    raw_txt: dict[str, str]


# ────────────────────────── advertise ───────────────────────────────

def advertise(
    peer_name: str,
    public_key_b64: str,
    *,
    port: int = DEFAULT_PEER_PORT,
    scope_default: str = "write-brain",
    zeroconf: "Zeroconf | None" = None,
) -> tuple["Zeroconf", "ServiceInfo"]:
    """Register this host's mDNS service for the Octopus mesh.

    Returns the owned ``Zeroconf`` + ``ServiceInfo`` so the caller can
    unregister on shutdown. If the caller passed in a pre-existing
    ``Zeroconf`` instance (e.g. the agent manager already has one) we
    reuse it; otherwise we create one and the caller owns the lifetime.

    Args:
        peer_name: mesh-wide name, becomes the instance label.
        public_key_b64: the peer's Ed25519 public key, base64(32 raw bytes).
        port: shim HTTP port. Default 8765 matches Phase 1's ``BIND_PORT``.
        scope_default: hint for remote invite UIs.
        zeroconf: optional pre-existing Zeroconf instance to register on.

    Raises:
        ImportError: zeroconf not installed.
        socket.error: binding the mDNS socket failed (multicast blocked,
            privilege required).
    """
    _require_zeroconf()
    if not peer_name or not peer_name.strip():
        raise ValueError("peer_name must be non-empty")
    instance = f"{peer_name.strip()}.{MAKAKOO_SERVICE_TYPE}"
    properties = {
        b"peer_name": peer_name.strip().encode("utf-8"),
        b"peer_pubkey": public_key_b64.encode("utf-8"),
        b"v": b"1",
        b"scope_default": scope_default.encode("utf-8"),
    }

    # Advertise on every non-loopback IPv4 interface. We serialize a
    # snapshot of addresses at registration time; if interfaces come/go
    # the caller is responsible for re-advertising.
    addresses = [socket.inet_aton(a) for a in _local_ipv4_addresses()]
    if not addresses:
        # Fall back to localhost — mDNS on loopback is useless but
        # keeping advertise() strict on at-least-one-iface crashes
        # laptops on airplane mode.
        addresses = [socket.inet_aton("127.0.0.1")]

    info = ServiceInfo(
        MAKAKOO_SERVICE_TYPE,
        instance,
        addresses=addresses,
        port=port,
        properties=properties,
        server=f"{peer_name.strip()}.local.",
    )
    zc = zeroconf or Zeroconf()
    zc.register_service(info, allow_name_change=True)
    log.info("mDNS advertise: %s at port %d (addresses=%d)",
             instance, port, len(addresses))
    return zc, info


def _local_ipv4_addresses() -> list[str]:
    """Return non-loopback IPv4 addresses of the host.

    Simple implementation: resolve ``socket.gethostname`` and walk its
    addrinfo. Missing a couple of rarely-used interfaces is fine —
    zeroconf will still pick up traffic on the canonical ones.
    """
    out: list[str] = []
    try:
        hostname = socket.gethostname()
        for info in socket.getaddrinfo(hostname, None, socket.AF_INET):
            addr = info[4][0]
            if addr and not addr.startswith("127."):
                out.append(addr)
    except socket.gaierror:
        pass
    return sorted(set(out))


# ────────────────────────── discover ────────────────────────────────

def discover(timeout_s: float = 3.0) -> list[DiscoveredPeer]:
    """Browse the LAN for Octopus peers, returning up to ``timeout_s`` worth.

    The timeout bounds the discovery window — we return whatever we've
    collected so far. Tune down for interactive UIs (cursor blinking)
    and up for CI where a slower network might delay the first response.

    Sprint criterion: "roundtrip: advertise on Node A found by discover
    on Node B within 3 seconds". Default matches that.

    Raises:
        ImportError: zeroconf not installed.
    """
    _require_zeroconf()
    zc = Zeroconf()
    try:
        collector = _PeerCollector()
        ServiceBrowser(zc, MAKAKOO_SERVICE_TYPE, collector)
        time.sleep(timeout_s)
        return collector.snapshot()
    finally:
        zc.close()


class _PeerCollector:  # pragma: no cover - exercised via integration
    """Zeroconf ServiceListener that materializes peers as they arrive."""

    def __init__(self) -> None:
        self._peers: dict[str, DiscoveredPeer] = {}
        self._zc: "Zeroconf | None" = None

    # zeroconf protocol methods
    def add_service(self, zc, type_, name):
        self._zc = zc
        self._resolve(zc, type_, name)

    def update_service(self, zc, type_, name):
        self._resolve(zc, type_, name)

    def remove_service(self, zc, type_, name):
        # Keep stale records in the snapshot? No — the point of
        # discovery is "who is reachable right now"; remove on leave.
        self._peers.pop(name, None)

    def _resolve(self, zc, type_, name):
        info = zc.get_service_info(type_, name)
        if not info:
            return
        addresses = [socket.inet_ntoa(a) for a in info.addresses or []]
        host = addresses[0] if addresses else "0.0.0.0"
        raw_txt: dict[str, str] = {}
        for k, v in (info.properties or {}).items():
            try:
                raw_txt[k.decode()] = v.decode() if v is not None else ""
            except Exception:
                continue
        self._peers[name] = DiscoveredPeer(
            peer_name=raw_txt.get("peer_name", name.split(".")[0]),
            host=host,
            port=info.port or DEFAULT_PEER_PORT,
            public_key_b64=raw_txt.get("peer_pubkey"),
            scope_default=raw_txt.get("scope_default"),
            raw_txt=raw_txt,
        )

    def snapshot(self) -> list[DiscoveredPeer]:
        return list(self._peers.values())


# ────────────────────────── Tytus CIDR scan ─────────────────────────

def tytus_cidr_scan(
    *,
    subnet: str = TYTUS_POD_SUBNET,
    port: int = DEFAULT_PEER_PORT,
    timeout_s: float = 0.2,
    max_workers: int = 16,
    probe: Callable[[str, int, float], bool] | None = None,
) -> list[str]:
    """Probe every host in the Tytus pod subnet for an HTTP shim.

    Returns the addresses that responded on ``port`` within
    ``timeout_s`` per probe. mDNS doesn't reach pods over WireGuard
    (multicast is blocked), so this CIDR sweep is the discovery path
    on the Tytus side. 254 addresses × 200 ms with 16 parallel workers
    completes in ~3 seconds worst case, which matches the sprint's
    mDNS-roundtrip budget.

    ``probe`` is injectable for tests: the default is :func:`_tcp_probe`
    which opens a plain TCP connection. Tests swap in a stub.

    Example:
        >>> tytus_cidr_scan()
        ['10.42.42.1', '10.42.42.4']  # only peers with a listening shim
    """
    probe = probe or _tcp_probe
    net = ipaddress.ip_network(subnet, strict=False)
    candidates = [str(h) for h in net.hosts()]
    alive: list[str] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=max_workers) as pool:
        futures = {pool.submit(probe, c, port, timeout_s): c for c in candidates}
        for fut in concurrent.futures.as_completed(futures):
            addr = futures[fut]
            try:
                if fut.result():
                    alive.append(addr)
            except Exception:
                # Probe failures (timeouts, resets) are the common case —
                # non-responders are the normal state for most of /24.
                pass
    alive.sort(key=lambda a: tuple(int(p) for p in a.split(".")))
    return alive


def _tcp_probe(host: str, port: int, timeout_s: float) -> bool:
    """Default probe: attempt TCP connect with ``timeout_s`` deadline.

    Does NOT speak HTTP — we only care whether a listener answered on
    the expected port. The actual shim health check is owned by
    :mod:`core.octopus.discovery.handshake`, which signs a real /rpc
    request and interprets the response.
    """
    try:
        with socket.create_connection((host, port), timeout=timeout_s):
            return True
    except (OSError, socket.timeout):
        return False

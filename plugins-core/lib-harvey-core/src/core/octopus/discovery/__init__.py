"""Peer discovery for Harvey Octopus.

Two paths converge on the same handshake:

- :mod:`mdns`     — LAN discovery via `_makakoo-peer._tcp.` service type.
                    Works for 2 Macs on the same Wi-Fi / SME office network.
- :mod:`invite`   — out-of-band invite URL (``makakoo://join?t=<base64>``).
                    Works across the internet, for Tytus pods, for QR
                    flows, and for corporate networks where multicast is
                    blocked.

Both feed :mod:`handshake` which performs the cryptographic
challenge-response using the onboarding token's shared secret and
returns the elevated TrustGrant via :mod:`core.octopus.trust_store`.

The Tytus integration (CIDR scan on 10.42.42.0/24) is a special case of
:mod:`mdns` that sweeps for known-port peers over the WireGuard tunnel
where multicast is blocked — see :func:`mdns.tytus_cidr_scan`.
"""

MAKAKOO_SERVICE_TYPE = "_makakoo-peer._tcp.local."
"""mDNS service type — reserved for Makakoo OS peer discovery."""

DEFAULT_PEER_PORT = 8765
"""Default TCP port the HTTP shim binds to. Advertised via mDNS TXT.
Stay in sync with ``core.mcp.http_shim.BIND_PORT``."""

TYTUS_POD_SUBNET = "10.42.42.0/24"
"""WireGuard tunnel subnet used by Tytus pods. Sweep targets 10.42.42.1,
10.42.42.2, ... up to 10.42.42.254 looking for the HTTP shim."""
